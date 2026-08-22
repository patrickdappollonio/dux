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
//! The match in [`owner_of_reaction`] is EXHAUSTIVE on purpose: a new reaction
//! variant does not compile until somebody has said whether it needs routing.
//!
//! ## Why the answer can be SNAPSHOT
//!
//! The web layer's own follow-ups REMOVE the pending-op entry they were routed
//! by: `drive_pr_lookup_followup` and `finish_web_project_add` both take their op
//! out of the map to resolve it. On the concurrent path the web fanout runs
//! BEFORE the drainer applies the reaction, so a verdict read from the live maps
//! after the fanout answers `Drainer` for work the web has already done, and the
//! terminal UI runs its arm too: a second name prompt, a second project add.
//!
//! So the concurrent drainer takes a [`WebFollowupOps`] snapshot BEFORE it lends
//! the reaction to the web layer and routes against that. The live-map form
//! ([`Engine::followup_owner`]) stays correct for `dux server` and the flip,
//! which ask before they drive.

use std::collections::HashSet;

use super::{Engine, EventReaction};

/// The web pending-op id sets the routing consults, read either live off the
/// engine or from a snapshot.
///
/// One trait so the exhaustive match in [`owner_of_reaction`] exists exactly once
/// and cannot drift between the live and snapshot forms.
pub trait WebFollowupOpsView {
    fn has_pr_lookup(&self, id: &str) -> bool;
    fn has_add_project(&self, id: &str) -> bool;
    fn has_checkout(&self, id: &str) -> bool;
}

impl WebFollowupOpsView for Engine {
    fn has_pr_lookup(&self, id: &str) -> bool {
        self.pending_web_pr_lookup_ops.contains_key(id)
    }
    fn has_add_project(&self, id: &str) -> bool {
        self.pending_web_add_project_ops.contains_key(id)
    }
    fn has_checkout(&self, id: &str) -> bool {
        self.pending_web_checkout_ops.contains_key(id)
    }
}

/// A point-in-time copy of the web pending-op ids, so an ownership verdict can be
/// decided before anything has had a chance to consume the entry it depends on.
///
/// Only the ids: the ops themselves are not `Clone` and nothing about routing
/// needs them. Empty in the single-surface case, where the sets are never taken.
#[derive(Debug, Clone, Default)]
pub struct WebFollowupOps {
    pr_lookup: HashSet<String>,
    add_project: HashSet<String>,
    checkout: HashSet<String>,
}

impl WebFollowupOps {
    /// Which surface owns the follow-up for `reaction`, per this snapshot.
    pub fn owner_of(&self, reaction: &EventReaction) -> FollowupOwner {
        owner_of_reaction(self, reaction)
    }
}

impl WebFollowupOpsView for WebFollowupOps {
    fn has_pr_lookup(&self, id: &str) -> bool {
        self.pr_lookup.contains(id)
    }
    fn has_add_project(&self, id: &str) -> bool {
        self.add_project.contains(id)
    }
    fn has_checkout(&self, id: &str) -> bool {
        self.checkout.contains(id)
    }
}

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
    /// Which surface owns the follow-up work for `reaction`, read from the LIVE
    /// pending-op maps.
    ///
    /// Correct for a surface that asks BEFORE it drives (`dux server` and the
    /// flip both do). The concurrent drainer must use a [`WebFollowupOps`]
    /// snapshot instead, because the web fanout it runs first removes the entries
    /// this would have consulted.
    pub fn followup_owner(&self, reaction: &EventReaction) -> FollowupOwner {
        owner_of_reaction(self, reaction)
    }

    /// Snapshot the web pending-op ids the routing consults, for a drainer that
    /// has to decide ownership before it lends the reaction to the web layer.
    pub fn web_followup_ops(&self) -> WebFollowupOps {
        WebFollowupOps {
            pr_lookup: self.pending_web_pr_lookup_ops.keys().cloned().collect(),
            add_project: self.pending_web_add_project_ops.keys().cloned().collect(),
            checkout: self.pending_web_checkout_ops.keys().cloned().collect(),
        }
    }
}

/// Which surface owns the follow-up work for `reaction`.
///
/// Only the reactions whose follow-up DOES something route; everything else
/// answers [`FollowupOwner::Drainer`], which in a single-surface process is the
/// only answer there has ever been. That is what keeps `dux server` and the flip
/// behaving exactly as before: their web pending-op maps hold the ids of their own
/// operations, so they get `Web` for their own work and `Drainer` for anything a
/// worker started on its own.
pub fn owner_of_reaction(ops: &impl WebFollowupOpsView, reaction: &EventReaction) -> FollowupOwner {
    match reaction {
            // A PR lookup resolved. The web dispatches the create straight away
            // because the browser already sent the name; the TUI opens a name
            // prompt. Both at once is a prompt the user did not ask for on top of
            // an agent that is already being created.
            EventReaction::OpenNewAgentPromptForPr { status_op_id, .. } => {
                owner_by_id(status_op_id, |id| ops.has_pr_lookup(id))
            }

            // Worker 2's `git switch` landed and the project is ready to add.
            // Adding it twice is two workspace entries for one directory.
            EventReaction::AddProjectAfterBranchCheckout { status_op_id, .. }
            | EventReaction::AddProjectAfterInitialCommit { status_op_id, .. } => {
                owner_by_id(status_op_id, |id| ops.has_add_project(id))
            }

            // Worker 1 inspected the default branch and the follow-up SPAWNS
            // worker 2. Two spawns are two concurrent `git switch` runs in one
            // repository.
            EventReaction::DispatchProjectDefaultBranchCheckout { status_op_id, .. } => {
                owner_by_id(status_op_id, |id| ops.has_checkout(id))
            }

            // A `Multi` is applied leaf by leaf (the TUI's `apply_reaction`
            // recurses, and the web's `drive_*` follow-ups match bare variants
            // only), so routing is decided per leaf where the leaf is handled.
            // Answering for the wrapper would skip unrelated siblings.
            EventReaction::Multi(_) => FollowupOwner::Drainer,

            // Everything below is safe to handle on both surfaces at once, and
            // each line says why rather than leaning on a wildcard.
            //
            // Statuses and clears: each surface renders its own copy. Two web
            // follow-ups are deliberately left UNROUTED and self-guard instead:
            // `drive_delete_followup` and `drive_web_launch_followup` both look
            // their session up in a web pending map first and do nothing when it
            // is absent, so a TUI-started delete or launch runs its web half as a
            // no-op. That is safe where a routed arm is not, because neither one
            // starts new work: they resolve a keyed op the web itself opened. The
            // routed arms above spawn a git job, add a project, or dispatch a
            // create, which is why they cannot rely on the same trick.
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
            // Both pre-flights' listeners are the TUI's to stash; the web has no
            // arm for either, and by construction it cannot: they only exist on a
            // surface that can bind before it hands anything over.
            | EventReaction::ServerFlipPreflightReady { .. }
            | EventReaction::BackgroundServerPreflightReady { .. } => FollowupOwner::Drainer,
    }
}

/// [`FollowupOwner::Web`] when the reaction carries an op id that `in_web_map`
/// finds, otherwise [`FollowupOwner::Drainer`].
///
/// A missing id is deliberately `Drainer` rather than an error: the TUI paths
/// pass `None` for operations they never keyed, and a worker that started
/// something on its own (a resume-fallback retry, say) has no originating
/// request at all.
fn owner_by_id(status_op_id: &Option<String>, in_web_map: impl Fn(&str) -> bool) -> FollowupOwner {
    match status_op_id.as_deref() {
        Some(id) if in_web_map(id) => FollowupOwner::Web,
        _ => FollowupOwner::Drainer,
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

    /// A snapshot keeps answering `Web` after the web layer's own follow-up has
    /// consumed the pending-op entry.
    ///
    /// This is the whole reason the snapshot exists: `drive_pr_lookup_followup`
    /// removes the op to resolve it, and the concurrent drainer asks AFTER that
    /// has run. Reading the live maps at that point says `Drainer` and the
    /// terminal UI pops a name prompt for an agent a browser already created.
    #[test]
    fn a_snapshot_survives_the_web_followup_consuming_its_op() {
        let (mut engine, _tmp) = engine();
        let op = status_op("Resolving PR…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebPrLookupOutcome| crate::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        engine.pending_web_pr_lookup_ops.insert(id.clone(), op);
        let snapshot = engine.web_followup_ops();
        let reaction = pr_reaction(Some(id.clone()));

        // Stand in for the web fanout: its follow-up takes the op out to resolve it.
        engine.pending_web_pr_lookup_ops.remove(&id);

        assert_eq!(
            engine.followup_owner(&reaction),
            FollowupOwner::Drainer,
            "the live maps forget, which is exactly the trap"
        );
        assert_eq!(
            snapshot.owner_of(&reaction),
            FollowupOwner::Web,
            "the snapshot must still name the web, or the drained reaction double-runs"
        );
    }

    /// The snapshot covers all three routed maps, not just the PR one.
    #[test]
    fn a_snapshot_covers_every_routed_map() {
        let (mut engine, _tmp) = engine();
        let add_op = status_op("Adding…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebAddProjectOutcome| crate::engine::Final::info("done"),
        );
        let add_id = add_op.id().to_string();
        engine
            .pending_web_add_project_ops
            .insert(add_id.clone(), add_op);
        let checkout_op = status_op("Checking out…".to_string()).resolve_in_handler(
            |_o: &crate::engine::WebCheckoutOutcome| crate::engine::Final::info("done"),
        );
        let checkout_id = checkout_op.id().to_string();
        engine
            .pending_web_checkout_ops
            .insert(checkout_id.clone(), checkout_op);
        let snapshot = engine.web_followup_ops();
        engine.pending_web_add_project_ops.remove(&add_id);
        engine.pending_web_checkout_ops.remove(&checkout_id);

        let add = EventReaction::AddProjectAfterBranchCheckout {
            path: "/tmp/p".to_string(),
            name: "p".to_string(),
            target_branch: "main".to_string(),
            leading_branch: "main".to_string(),
            status_op_id: Some(add_id),
        };
        let checkout = EventReaction::DispatchProjectDefaultBranchCheckout {
            project: sample_project("p1", "/tmp/p1"),
            default_branch: "main".to_string(),
            status_op_id: Some(checkout_id),
        };
        assert_eq!(snapshot.owner_of(&add), FollowupOwner::Web);
        assert_eq!(snapshot.owner_of(&checkout), FollowupOwner::Web);
    }

    /// An empty snapshot (nothing web-originated in flight) routes everything to
    /// the drainer, so a terminal-only workspace is unaffected by the mechanism.
    #[test]
    fn an_empty_snapshot_routes_everything_to_the_drainer() {
        let (engine, _tmp) = engine();
        let snapshot = engine.web_followup_ops();
        assert_eq!(
            snapshot.owner_of(&pr_reaction(Some("an-op".to_string()))),
            FollowupOwner::Drainer
        );
    }
}
