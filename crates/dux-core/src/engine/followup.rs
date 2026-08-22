//! Which surface owns the side-effecting follow-up for a drained worker-event
//! reaction.
//!
//! Both surfaces read the SAME worker-event stream when the web server serves in
//! the background of a running terminal UI: the TUI drains `worker_rx` and hands
//! each reaction to the web layer before applying it itself. Most reactions are
//! harmless on both sides (a status is rendered once per surface, a view refresh
//! touches only its own surface), but a handful DO something: they spawn a git
//! job, add a project to the workspace, or dispatch an agent create. Running one
//! of those twice checks out a branch twice, adds a project twice, and pops a
//! terminal name prompt for an agent a browser already asked to create.
//!
//! So those reactions are origin-routed. Every web-originated operation stashes a
//! keyed [`crate::engine::HandlerStatusOp`] in one of the engine's web pending-op
//! maps and forwards that op's opaque id into the worker; the id travels back on
//! the reaction. If a web map holds the id, the web layer owns the follow-up and
//! the TUI must not act. If nothing holds it, the operation came from whichever
//! surface drained the event, and that surface owns it.
//!
//! The match in [`Engine::followup_owner`] is EXHAUSTIVE on purpose: a new
//! reaction variant does not compile until somebody has said whether it needs
//! routing.

use super::{Engine, EventReaction};

/// The surface that owns the side-effecting follow-up for one reaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowupOwner {
    /// A web client started this operation: the id the reaction carries is in one
    /// of the engine's web pending-op maps. The web layer's `drive_*` follow-up
    /// runs it, and the surface that drained the event skips its own arm.
    Web,
    /// Nothing claims this reaction, so the surface that drained the worker event
    /// owns the follow-up. That is the web layer for `dux server` and the flip,
    /// and the terminal UI while the background server is on.
    Drainer,
}

impl Engine {
    /// Which surface owns the follow-up work for `reaction`.
    ///
    /// Only the reactions whose follow-up DOES something route; everything else
    /// answers [`FollowupOwner::Drainer`], which in a single-surface process is
    /// the only answer there has ever been. That is what keeps `dux server` and
    /// the flip behaving exactly as before: their web pending-op maps hold the
    /// ids of their own operations, so they get `Web` for their own work and
    /// `Drainer` for anything a worker started on its own.
    pub fn followup_owner(&self, reaction: &EventReaction) -> FollowupOwner {
        match reaction {
            // A PR lookup resolved. The web dispatches the create straight away
            // because the browser already sent the name; the TUI opens a name
            // prompt. Both at once is a prompt the user did not ask for on top of
            // an agent that is already being created.
            EventReaction::OpenNewAgentPromptForPr { status_op_id, .. } => {
                self.owner_of(status_op_id, |id| {
                    self.pending_web_pr_lookup_ops.contains_key(id)
                })
            }

            // Worker 2's `git switch` landed and the project is ready to add.
            // Adding it twice is two workspace entries for one directory.
            EventReaction::AddProjectAfterBranchCheckout { status_op_id, .. }
            | EventReaction::AddProjectAfterInitialCommit { status_op_id, .. } => {
                self.owner_of(status_op_id, |id| {
                    self.pending_web_add_project_ops.contains_key(id)
                })
            }

            // Worker 1 inspected the default branch and the follow-up SPAWNS
            // worker 2. Two spawns are two concurrent `git switch` runs in one
            // repository.
            EventReaction::DispatchProjectDefaultBranchCheckout { status_op_id, .. } => {
                self.owner_of(status_op_id, |id| {
                    self.pending_web_checkout_ops.contains_key(id)
                })
            }

            // A `Multi` is applied leaf by leaf (the TUI's `apply_reaction`
            // recurses, and the web's `drive_*` follow-ups match bare variants
            // only), so routing is decided per leaf where the leaf is handled.
            // Answering for the wrapper would skip unrelated siblings.
            EventReaction::Multi(_) => FollowupOwner::Drainer,

            // Everything below is safe to handle on both surfaces at once, and
            // each line says why rather than leaning on a wildcard.
            //
            // Statuses and clears: each surface renders its own copy. Web
            // follow-ups that only RESOLVE a keyed op (delete, launch) are
            // self-guarding for the same reason the routed arms are not: they
            // look their id up in a web map first and do nothing when it is
            // absent.
            EventReaction::Nothing
            | EventReaction::Status(_)
            | EventReaction::ClearStatus(_)
            // View refreshes touch only the surface that runs them.
            | EventReaction::RebuildLeftItems
            | EventReaction::ReloadChangedFiles
            | EventReaction::ClampFilesCursor
            // Launch outcomes: the web arm resolves its own pending launch op
            // (keyed by session id, present only for a web-started launch) and
            // the TUI arm updates its own view of the same agent. Both must run.
            | EventReaction::AgentLaunchReadyView(_)
            | EventReaction::AgentLaunchFailedView(_)
            // Picker and browser payloads: TUI-only overlays. The web reads the
            // same core functions through its own routes.
            | EventReaction::BrowserEntriesArrived { .. }
            | EventReaction::ProjectWorktreesArrived { .. }
            | EventReaction::ManageableWorktreesArrived { .. }
            | EventReaction::StartupLogsArrived { .. }
            | EventReaction::StartupLogContentArrived { .. }
            | EventReaction::ResourceStatsArrived(..)
            // Deletion: the web arm resolves a pending delete op keyed by session
            // id and present only for a web-started delete; the TUI arm updates
            // its own selection and message.
            | EventReaction::WorktreeRemoveSucceeded { .. }
            | EventReaction::WorktreeRemoveFailed { .. }
            | EventReaction::FinishDeleteSessionView(_)
            | EventReaction::DoDeleteSessionView(_)
            | EventReaction::BeginDeleteSessionView(_)
            // A create-agent branch inspection continuing. The web has no
            // follow-up for it at all (it validates and dispatches inside its own
            // request), so there is nothing to double-run.
            | EventReaction::ContinueCreateAgentAfterInspection { .. }
            // Config reload: the drainer applies the config, and the web layer's
            // hook separately fires the `config.changed` emit its own reload arm
            // used to. Neither mutates what the other does.
            | EventReaction::ApplyReloadedConfig(_)
            | EventReaction::OpenConfigReloadFailedModal(_)
            // Project persistence: the engine already performed the mutation;
            // both arms only report and mirror it.
            | EventReaction::ProjectPersistenceOutcome(_)
            // Launch/terminal dispatch views: the engine did the work, the arms
            // describe it to their own surface.
            | EventReaction::DispatchAgentLaunchView(_)
            | EventReaction::DeleteTerminalView(_)
            // The flip pre-flight's listeners are the TUI's to stash; the web has
            // no arm for it.
            | EventReaction::ServerFlipPreflightReady { .. } => FollowupOwner::Drainer,
        }
    }

    /// [`FollowupOwner::Web`] when the reaction carries an op id that `in_web_map`
    /// finds, otherwise [`FollowupOwner::Drainer`].
    ///
    /// A missing id is deliberately `Drainer` rather than an error: the TUI paths
    /// pass `None` for operations they never keyed, and a worker that started
    /// something on its own (a resume-fallback retry, say) has no originating
    /// request at all.
    fn owner_of(
        &self,
        status_op_id: &Option<String>,
        in_web_map: impl Fn(&str) -> bool,
    ) -> FollowupOwner {
        match status_op_id.as_deref() {
            Some(id) if in_web_map(id) => FollowupOwner::Web,
            _ => FollowupOwner::Drainer,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FollowupOwner;
    use crate::engine::test_support::{sample_project, test_engine};
    use crate::engine::{Engine, EventReaction, status_op};

    /// A resolved PR carrying `pr_op` as the reaction's op id, so a routing test
    /// only has to say whether that id is in a web map.
    fn pr_reaction(status_op_id: Option<String>) -> EventReaction {
        EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(crate::worker::ResolvedPullRequest {
                project: sample_project("p1", "/tmp/p1"),
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                number: 42,
                title: "Fix bug".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "feature/pr-42".to_string(),
                custom_name: Some("my-agent".to_string()),
            }),
            status_op_id,
        }
    }

    fn engine() -> (Engine, tempfile::TempDir) {
        test_engine()
    }

    /// A web-originated PR lookup routes to the web, which dispatches the create
    /// with the name the browser already sent. The TUI must not open its name
    /// prompt for it: that was the concrete bug this routing exists to stop.
    #[test]
    fn a_web_originated_pr_lookup_routes_to_the_web() {
        let (mut engine, _tmp) = engine();
        let op = status_op("Resolving PR…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebPrLookupOutcome| crate::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        engine.pending_web_pr_lookup_ops.insert(id.clone(), op);
        assert_eq!(
            engine.followup_owner(&pr_reaction(Some(id))),
            FollowupOwner::Web
        );
    }

    /// The same reaction with an id no web map knows is the TUI's own lookup, so
    /// it routes to whichever surface drained it: the prompt opens.
    #[test]
    fn a_tui_originated_pr_lookup_routes_to_the_drainer() {
        let (engine, _tmp) = engine();
        assert_eq!(
            engine.followup_owner(&pr_reaction(Some("a-tui-op-id".to_string()))),
            FollowupOwner::Drainer
        );
    }

    /// No id at all: a worker started this on its own, so the drainer owns it.
    #[test]
    fn an_unowned_reaction_routes_to_the_drainer() {
        let (engine, _tmp) = engine();
        assert_eq!(
            engine.followup_owner(&pr_reaction(None)),
            FollowupOwner::Drainer
        );
    }

    /// The checkout hand-off spawns a git job, so it is routed by the checkout
    /// map rather than by the add-project one. Cross-wiring the maps would send
    /// every checkout to the drainer and double-spawn worker 2.
    #[test]
    fn a_web_originated_default_branch_checkout_routes_to_the_web() {
        let (mut engine, _tmp) = engine();
        let op = status_op("Checking out…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebCheckoutOutcome| crate::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        engine.pending_web_checkout_ops.insert(id.clone(), op);
        let reaction = EventReaction::DispatchProjectDefaultBranchCheckout {
            project: sample_project("p1", "/tmp/p1"),
            default_branch: "main".to_string(),
            status_op_id: Some(id.clone()),
        };
        assert_eq!(engine.followup_owner(&reaction), FollowupOwner::Web);

        // And the add-project map is NOT consulted for it.
        engine.pending_web_checkout_ops.remove(&id);
        assert_eq!(engine.followup_owner(&reaction), FollowupOwner::Drainer);
    }

    /// Both add-project hand-offs read the add-project map, because both of them
    /// perform the same inline add.
    #[test]
    fn both_add_project_handoffs_route_by_the_add_project_map() {
        let (mut engine, _tmp) = engine();
        let op = status_op("Adding…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebAddProjectOutcome| crate::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        engine.pending_web_add_project_ops.insert(id.clone(), op);
        let after_checkout = EventReaction::AddProjectAfterBranchCheckout {
            path: "/tmp/p".to_string(),
            name: "p".to_string(),
            target_branch: "main".to_string(),
            leading_branch: "main".to_string(),
            status_op_id: Some(id.clone()),
        };
        let after_commit = EventReaction::AddProjectAfterInitialCommit {
            path: "/tmp/p".to_string(),
            name: "p".to_string(),
            branch: "main".to_string(),
            leading_branch: "main".to_string(),
            initialized_repo: true,
            seeded_gitignore: false,
            seed_warning: None,
            status_op_id: Some(id),
        };
        assert_eq!(engine.followup_owner(&after_checkout), FollowupOwner::Web);
        assert_eq!(engine.followup_owner(&after_commit), FollowupOwner::Web);
    }

    /// A reaction both surfaces are meant to handle stays with the drainer even
    /// while web ops are open, so a browser-driven delete still updates the TUI's
    /// sidebar and a TUI-driven one still resolves the web's toast.
    #[test]
    fn reactions_both_surfaces_handle_stay_with_the_drainer() {
        let (engine, _tmp) = engine();
        for reaction in [
            EventReaction::Nothing,
            EventReaction::RebuildLeftItems,
            EventReaction::ClearStatus("k".to_string()),
            EventReaction::Multi(vec![EventReaction::Nothing]),
            EventReaction::WorktreeRemoveFailed {
                session_id: "s1".to_string(),
                message: "boom".to_string(),
            },
        ] {
            assert_eq!(
                engine.followup_owner(&reaction),
                FollowupOwner::Drainer,
                "this reaction is handled on both surfaces"
            );
        }
    }
}
