//! The operation registry: what a client that asked for a change can learn
//! about how it really ended.
//!
//! A route answers before its change is done whenever the work runs on a
//! worker (a worktree removal, a create, a launch). The status line tells the
//! people watching a screen how it ended; this registry tells the client that
//! asked, by id, for as long as the record is kept. It is memory only, owned
//! by the engine, and lost on a restart, which is right: every connection that
//! could ask about it is lost too.
//!
//! # Ids
//!
//! A record's id is the keyed-status key the engine already mints for the
//! change, the same `op_id` the create routes have always answered with. A
//! change that runs to its end inside the call, or whose status rides a key
//! that is not unique to it (a stop's per-agent key, a tab launch's per-tab
//! key), gets a fresh id from the same minter, so the two never collide.
//!
//! # Completion points
//!
//! A record finishes in exactly one of these places, each named here so a new
//! route has a list to join:
//!
//! - Inside the call, on the engine thread, when the change finished before
//!   the route answered (`Engine::apply_wire_operation` and the tab and
//!   terminal arms of the web engine actor). Its message is the final the call
//!   produced, now carrying the record's id as its key.
//! - When the keyed final the record waits on lands where the web raises
//!   statuses (`StatusEmitter::send` and `StatusEmitter::clear` in
//!   `dux-web`'s engine actor). Every worker-run change on a covered route ends
//!   there: an agent create (new, fork, from a worktree, standalone and, after
//!   the pull-request lookup hands off, from a pull request), an agent delete
//!   that removes its worktree, a stop that waits for the agent to exit, a
//!   start that launches, a project removal that deletes worktrees, and the
//!   project adds that run git first.
//! - When a tab's launch reports back (`Engine::drive_web_launch_followup`),
//!   for a tab created or started by a route, because the launch's own status
//!   says nothing about which request it answers.
//! - When the pull-request lookup behind a from-PR create fails without a
//!   keyed final of its own (`Engine::drive_pr_lookup_followup`).
//!
//! Route by route (all under `/api/v1`):
//!
//! | Route | Ends |
//! |---|---|
//! | `POST /projects`, a plain add | inside the call |
//! | `POST /projects`, an add that runs git first | its add op's final, at the emitter |
//! | `DELETE /projects/{id}` | inside the call when no worktree is removed, else the deletion op's final, at the emitter |
//! | `POST /sessions` (every kind) | the create op's final, at the emitter; a from-PR create follows its lookup, which hands off to the create or ends at the lookup follow-up |
//! | `DELETE /sessions/{id}` | inside the call when no worktree is removed, else the delete op's final, at the emitter |
//! | `POST /sessions/{id}/kill` | inside the call when nothing runs, else the stop's final, at the emitter |
//! | `POST /sessions/{id}/reconnect` | inside the call when nothing launches, else the launch op's final, at the emitter |
//! | `POST /sessions/{id}/tabs` | the new tab's launch report |
//! | `DELETE /sessions/{id}/tabs/{tab}` | inside the call |
//! | `POST /sessions/{id}/tabs/{tab}/start` | inside the call when the tab already runs, else its launch report |
//! | `POST` and `DELETE` on the three terminal addresses | inside the call |
//!
//! A record still running past its policy's `unknown_after` reads as
//! [`OperationState::Unknown`] but stays open, and finishes with the real
//! outcome whenever the work ends. Nothing here ever finishes a record on a
//! timer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::prose::ProseSegment;
use crate::statusline::StatusTone;

/// What kind of change a record follows. Spelled `noun.verb` on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum OperationKind {
    #[serde(rename = "project.add")]
    ProjectAdd,
    #[serde(rename = "project.remove")]
    ProjectRemove,
    #[serde(rename = "agent.create")]
    AgentCreate,
    #[serde(rename = "agent.delete")]
    AgentDelete,
    #[serde(rename = "agent.stop")]
    AgentStop,
    #[serde(rename = "agent.start")]
    AgentStart,
    #[serde(rename = "tab.create")]
    TabCreate,
    #[serde(rename = "tab.close")]
    TabClose,
    #[serde(rename = "tab.start")]
    TabStart,
    #[serde(rename = "terminal.create")]
    TerminalCreate,
    #[serde(rename = "terminal.close")]
    TerminalClose,
}

/// Where a record stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    /// The work is still running.
    Running,
    /// Everything the change set out to do was done.
    Succeeded,
    /// Nothing the change set out to do was done.
    Failed,
    /// Some of it was done and some was not; the parts say which.
    Partial,
    /// Still running past the policy's `unknown_after`. Not an outcome: the
    /// record stays open and finishes with the real one.
    Unknown,
}

impl OperationState {
    /// Whether the record has an outcome. `Unknown` has none yet.
    pub fn is_final(self) -> bool {
        !matches!(self, OperationState::Running | OperationState::Unknown)
    }
}

/// Which piece of a change a part reports on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    Worktree,
    Branch,
}

/// What became of one piece of a change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartOutcome {
    /// A worktree taken off disk.
    Removed,
    /// Left where it was, because nobody asked for it to go or it was not
    /// dux's to remove.
    Kept,
    /// A branch git deleted.
    Deleted,
    /// A branch that was already gone.
    AlreadyGone,
    /// A branch git refused to delete; it is still there.
    Refused,
    /// A removal that was tried and did not happen.
    Failed,
}

impl PartOutcome {
    fn done(self) -> bool {
        matches!(
            self,
            PartOutcome::Removed | PartOutcome::Deleted | PartOutcome::AlreadyGone
        )
    }

    fn missed(self) -> bool {
        matches!(self, PartOutcome::Refused | PartOutcome::Failed)
    }
}

/// One piece of a change and what became of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationPart {
    pub part: PartKind,
    /// The worktree's path or the branch's name.
    pub subject: String,
    pub outcome: PartOutcome,
    /// Why, when it was refused or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The facts a change reports beside its sentence: the ids it created and
/// removed, and what became of each piece.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperationNotes {
    pub created: Vec<String>,
    pub removed: Vec<String>,
    pub parts: Vec<OperationPart>,
}

impl OperationNotes {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.removed.is_empty() && self.parts.is_empty()
    }

    fn merge(&mut self, other: OperationNotes) {
        for id in other.created {
            if !self.created.contains(&id) {
                self.created.push(id);
            }
        }
        for id in other.removed {
            if !self.removed.contains(&id) {
                self.removed.push(id);
            }
        }
        self.parts.extend(other.parts);
    }

    /// The outcome these notes and a final of `tone` add up to. Something
    /// missed (a refused or failed part, or a final that is not an info) with
    /// something done is partial; missed with nothing done is a failure.
    fn settle(&self, tone: StatusTone) -> OperationState {
        let missed = self.parts.iter().any(|p| p.outcome.missed())
            || matches!(tone, StatusTone::Warning | StatusTone::Error);
        if !missed {
            return OperationState::Succeeded;
        }
        let did = !self.created.is_empty()
            || !self.removed.is_empty()
            || self.parts.iter().any(|p| p.outcome.done());
        if did {
            OperationState::Partial
        } else {
            OperationState::Failed
        }
    }
}

/// How long a record may run before it reads as unknown, and how long a
/// finished one is kept. Read from `[server]` when the record opens, so a
/// config reload applies to the operations started after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationPolicy {
    pub unknown_after: Duration,
    pub retention: Duration,
}

impl OperationPolicy {
    pub fn from_server(server: &crate::config::ServerConfig) -> Self {
        Self {
            unknown_after: Duration::from_secs(server.operation_unknown_after_seconds),
            retention: Duration::from_secs(server.operation_retention_seconds),
        }
    }
}

/// A record as a client reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationView {
    pub id: String,
    pub kind: OperationKind,
    pub state: OperationState,
    /// The final's sentence; empty while running, and for a change that ended
    /// without one.
    pub message: String,
    /// The parts `message` was built from, as the status stream carries them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<ProseSegment>>,
    pub created: Vec<String>,
    pub removed: Vec<String>,
    pub parts: Vec<OperationPart>,
}

/// The key a route binds a tab create or start to, finished when that tab's
/// launch reports back. Not a status key: a tab launch's status rides a key
/// shared by every launch of that tab, which cannot name one request.
pub fn launch_binding_key(tab_id: &str) -> String {
    format!("launch:{tab_id}")
}

/// The key a failed launch of an agent's first tab moves its records to. That
/// failure is reported by agent, not by tab, and a promotion may have made
/// another tab the first by the time it lands, so the engine moves the waiting
/// records off the tab that failed while it still knows which one that was.
pub fn launch_failure_key(session_id: &str) -> String {
    format!("launch-failed:{session_id}")
}

/// A fresh record id, from the same minter as every keyed status, so an id
/// minted here can never name another change's status.
pub fn mint_operation_id() -> String {
    crate::engine::status_op::next_status_id()
}

struct End {
    state: OperationState,
    message: String,
    segments: Option<Vec<ProseSegment>>,
    at: Instant,
}

struct Record {
    kind: OperationKind,
    policy: OperationPolicy,
    started: Instant,
    /// The key whose final finishes this record, while it runs.
    awaiting: Option<String>,
    notes: OperationNotes,
    end: Option<End>,
}

#[derive(Default)]
struct Registry {
    records: HashMap<String, Record>,
}

impl Registry {
    fn prune(&mut self, now: Instant) {
        self.records.retain(|_, record| match &record.end {
            Some(end) => now.saturating_duration_since(end.at) < record.policy.retention,
            None => true,
        });
    }

    /// The running records a final under `key` finishes: those waiting on it.
    fn awaiting(&mut self, key: &str) -> impl Iterator<Item = &mut Record> {
        self.records
            .values_mut()
            .filter(move |record| record.end.is_none() && record.awaiting.as_deref() == Some(key))
    }
}

/// The registry. Cheaply cloned; every clone is the same registry. A poisoned
/// lock is recovered rather than propagated: a panic elsewhere must not take a
/// client's answer down with it.
#[derive(Clone, Default)]
pub struct Operations(Arc<Mutex<Registry>>);

impl std::fmt::Debug for Operations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Operations")
            .field(&self.with(|r| r.records.len()))
            .finish()
    }
}

impl Operations {
    fn with<T>(&self, f: impl FnOnce(&mut Registry) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    /// How many records are held, running and finished. For tests and
    /// diagnostics: a leak shows up here as a number that only grows.
    pub fn len(&self) -> usize {
        self.with(|registry| registry.records.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Open a running record under `id`, waiting on a final keyed `id`.
    pub fn open(&self, id: &str, kind: OperationKind, policy: OperationPolicy, now: Instant) {
        self.with(|registry| {
            registry.prune(now);
            registry.records.insert(
                id.to_string(),
                Record {
                    kind,
                    policy,
                    started: now,
                    awaiting: Some(id.to_string()),
                    notes: OperationNotes::default(),
                    end: None,
                },
            );
        });
    }

    /// Forget a record whose change was refused before it started.
    pub fn discard(&self, id: &str) {
        self.with(|registry| {
            registry.records.remove(id);
        });
    }

    /// Move a record to a new id, before anybody was told the old one. A
    /// record waiting on its own id waits on the new one.
    pub fn rekey(&self, from: &str, to: &str) {
        self.with(|registry| {
            if let Some(mut record) = registry.records.remove(from) {
                if record.awaiting.as_deref() == Some(from) {
                    record.awaiting = Some(to.to_string());
                }
                registry.records.insert(to.to_string(), record);
            }
        });
    }

    /// Make the running record `id` wait on a final under `key` instead.
    pub fn await_key(&self, id: &str, key: &str) {
        self.with(|registry| {
            if let Some(record) = registry.records.get_mut(id)
                && record.end.is_none()
            {
                record.awaiting = Some(key.to_string());
            }
        });
    }

    /// Every record waiting on `from` now waits on `to`: the work behind
    /// `from` handed its outcome to a later operation.
    pub fn hand_off(&self, from: &str, to: &str) {
        self.with(|registry| {
            for record in registry.awaiting(from) {
                record.awaiting = Some(to.to_string());
            }
        });
    }

    /// Add facts to the records waiting on `key`, or to the record whose id it
    /// is.
    pub fn note(&self, key: &str, notes: OperationNotes) {
        if notes.is_empty() {
            return;
        }
        self.with(|registry| {
            let ids: Vec<String> = registry
                .records
                .iter()
                .filter(|(id, record)| {
                    id.as_str() == key
                        || (record.end.is_none() && record.awaiting.as_deref() == Some(key))
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                if let Some(record) = registry.records.get_mut(&id) {
                    record.notes.merge(notes.clone());
                }
            }
        });
    }

    /// Take `id` back out of what the records waiting on `key` created: the
    /// change made it and then lost it before it ended.
    pub fn retract_created(&self, key: &str, id: &str) {
        self.with(|registry| {
            for record in registry.awaiting(key) {
                record.notes.created.retain(|created| created != id);
            }
        });
    }

    /// A final of `tone` landed under `key`: finish every record waiting on
    /// it, its state settled from the tone and the record's notes.
    pub fn finish_by_key(
        &self,
        key: &str,
        tone: StatusTone,
        message: &str,
        segments: Option<&[ProseSegment]>,
        now: Instant,
    ) {
        self.with(|registry| {
            for record in registry.awaiting(key) {
                let state = record.notes.settle(tone);
                record.end = Some(End {
                    state,
                    message: message.to_string(),
                    segments: segments.map(<[ProseSegment]>::to_vec),
                    at: now,
                });
                record.awaiting = None;
            }
        });
    }

    /// Finish every record waiting on `key` with a state its completion point
    /// knows better than a status tone can say.
    pub fn finish_awaiting_as(
        &self,
        key: &str,
        state: OperationState,
        message: &str,
        segments: Option<&[ProseSegment]>,
        now: Instant,
    ) {
        self.with(|registry| {
            for record in registry.awaiting(key) {
                record.end = Some(End {
                    state,
                    message: message.to_string(),
                    segments: segments.map(<[ProseSegment]>::to_vec),
                    at: now,
                });
                record.awaiting = None;
            }
        });
    }

    /// Finish record `id` itself, with a final of `tone`.
    pub fn finish(
        &self,
        id: &str,
        tone: StatusTone,
        message: &str,
        segments: Option<&[ProseSegment]>,
        now: Instant,
    ) {
        self.with(|registry| {
            if let Some(record) = registry.records.get_mut(id)
                && record.end.is_none()
            {
                let state = record.notes.settle(tone);
                record.end = Some(End {
                    state,
                    message: message.to_string(),
                    segments: segments.map(<[ProseSegment]>::to_vec),
                    at: now,
                });
                record.awaiting = None;
            }
        });
    }

    /// Forget the finished records whose retention has passed by `now`.
    pub fn prune(&self, now: Instant) {
        self.with(|registry| registry.prune(now));
    }

    /// The record `id` as a client reads it at `now`, or `None` when there is
    /// none (never opened, or finished longer ago than its retention).
    pub fn view(&self, id: &str, now: Instant) -> Option<OperationView> {
        self.prune(now);
        self.peek(id, now)
    }

    /// [`Self::view`] without forgetting anything first: the snapshot a route
    /// answers with, taken where the change ended, so it holds whatever the
    /// retention is.
    pub fn peek(&self, id: &str, now: Instant) -> Option<OperationView> {
        self.with(|registry| {
            let record = registry.records.get(id)?;
            let (state, message, segments) = match &record.end {
                Some(end) => (end.state, end.message.clone(), end.segments.clone()),
                None if now.saturating_duration_since(record.started)
                    >= record.policy.unknown_after =>
                {
                    (OperationState::Unknown, String::new(), None)
                }
                None => (OperationState::Running, String::new(), None),
            };
            Some(OperationView {
                id: id.to_string(),
                kind: record.kind,
                state,
                message,
                segments,
                created: record.notes.created.clone(),
                removed: record.notes.removed.clone(),
                parts: record.notes.parts.clone(),
            })
        })
    }
}

/// The ids a change can create or remove, as they stood at one moment.
struct IdSnapshot {
    projects: Vec<String>,
    sessions: Vec<String>,
    terminals: Vec<String>,
}

impl IdSnapshot {
    fn of(engine: &crate::engine::Engine) -> Self {
        let mut terminals: Vec<String> = engine.companion_terminals.keys().cloned().collect();
        terminals.sort();
        Self {
            projects: engine.projects.iter().map(|p| p.id.clone()).collect(),
            sessions: engine.sessions.iter().map(|s| s.id.clone()).collect(),
            terminals,
        }
    }

    /// What appeared and what went between `self` and `after`.
    fn changes_to(&self, after: &IdSnapshot) -> OperationNotes {
        let mut notes = OperationNotes::default();
        for (before, now) in [
            (&self.projects, &after.projects),
            (&self.sessions, &after.sessions),
            (&self.terminals, &after.terminals),
        ] {
            notes
                .created
                .extend(now.iter().filter(|id| !before.contains(id)).cloned());
            notes
                .removed
                .extend(before.iter().filter(|id| !now.contains(id)).cloned());
        }
        notes
    }
}

/// The key a dispatched command's work is still running under, if it is: the
/// create op it minted, or the key of the busy it answered with.
fn running_key(outcome: &crate::wire::WireCommandOutcome) -> Option<String> {
    outcome.created_op_id.clone().or_else(|| {
        outcome
            .status
            .as_ref()
            .filter(|status| StatusTone::from_wire(&status.tone) == StatusTone::Busy)
            .and_then(|status| status.key.clone())
    })
}

/// What an agent delete did to the worktree and its branches, from the
/// removal the delete reports.
pub fn agent_delete_parts(
    directory: &str,
    branch: &str,
    initial_branch: &str,
    removal: &crate::engine::WorktreeRemoval,
) -> Vec<OperationPart> {
    use crate::engine::{RemovedBranches, WorktreeRemoval};
    let worktree = |outcome| OperationPart {
        part: PartKind::Worktree,
        subject: directory.to_string(),
        outcome,
        reason: None,
    };
    let branch_part = |name: &str, deletion: &crate::git::BranchDeletion| OperationPart {
        part: PartKind::Branch,
        subject: name.to_string(),
        outcome: match deletion {
            crate::git::BranchDeletion::Deleted => PartOutcome::Deleted,
            crate::git::BranchDeletion::AlreadyGone => PartOutcome::AlreadyGone,
            crate::git::BranchDeletion::Refused { .. } => PartOutcome::Refused,
        },
        reason: deletion.refused_reason().map(str::to_string),
    };
    let kept_branch = || OperationPart {
        part: PartKind::Branch,
        subject: branch.to_string(),
        outcome: PartOutcome::Kept,
        reason: None,
    };
    match removal {
        WorktreeRemoval::NothingToRemove { .. } => Vec::new(),
        WorktreeRemoval::PreservedShared
        | WorktreeRemoval::PreservedOrphan
        | WorktreeRemoval::SkippedForSiblings => {
            let mut parts = vec![worktree(PartOutcome::Kept)];
            if !branch.is_empty() {
                parts.push(kept_branch());
            }
            parts
        }
        WorktreeRemoval::Performed {
            branches: RemovedBranches::Kept(_),
        } => vec![worktree(PartOutcome::Removed), kept_branch()],
        WorktreeRemoval::Performed {
            branches: RemovedBranches::Deleted(result),
        } => {
            let mut parts = vec![
                worktree(PartOutcome::Removed),
                branch_part(branch, &result.branch),
            ];
            if let Some(initial) = &result.initial_branch {
                parts.push(branch_part(initial_branch, initial));
            }
            parts
        }
    }
}

/// What an agent delete whose worktree removal failed left behind: the
/// worktree, still on disk, and its branch, never reached.
pub fn agent_delete_failure_parts(
    directory: &str,
    branch: &str,
    reason: &str,
) -> Vec<OperationPart> {
    let mut parts = vec![OperationPart {
        part: PartKind::Worktree,
        subject: directory.to_string(),
        outcome: PartOutcome::Failed,
        reason: Some(reason.to_string()),
    }];
    if !branch.is_empty() {
        parts.push(OperationPart {
            part: PartKind::Branch,
            subject: branch.to_string(),
            outcome: PartOutcome::Kept,
            reason: None,
        });
    }
    parts
}

impl crate::engine::Engine {
    /// Dispatch `command` as a change a client will ask about, and open its
    /// operation record. The record's id comes back as the outcome's
    /// `operation_id`.
    ///
    /// The record is opened here, on the engine thread, before the command's
    /// statuses are raised, so no final can land before the record waits for
    /// it. A command that finished inside this call is the first completion
    /// point: its record ends now, on the final the command answered with,
    /// which carries the record's id as its key when it had none. A command
    /// still running keeps its record open under the key of its create op or
    /// its busy; a key minted for this one operation becomes the record's id,
    /// and a key other changes share is waited on under a fresh id.
    ///
    /// A command that is refused outright (an `Err`) leaves no record.
    pub fn apply_wire_operation(
        &mut self,
        command: crate::wire::WireCommand,
        kind: OperationKind,
    ) -> anyhow::Result<crate::wire::WireCommandOutcome> {
        let now = Instant::now();
        let provisional = mint_operation_id();
        self.open_operation(&provisional, kind);
        let before = IdSnapshot::of(self);
        self.operation_in_dispatch = Some(provisional.clone());
        let result = self.apply_wire(command);
        self.operation_in_dispatch = None;
        let mut outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.operations.discard(&provisional);
                return Err(error);
            }
        };
        let deferred = self
            .deferred_operations
            .iter()
            .any(|op| op.as_deref() == Some(provisional.as_str()));
        let id = if deferred {
            // Held behind a config reload: nothing has happened yet, so the
            // record waits for the drain that runs it.
            provisional
        } else {
            self.operations
                .note(&provisional, before.changes_to(&IdSnapshot::of(self)));
            let settled = outcome.settled.clone();
            self.settle_operation(
                &provisional,
                running_key(&outcome),
                outcome.status.as_mut(),
                settled.as_ref(),
                true,
                now,
            )
        };
        outcome.operation = self.operations.peek(&id, now).map(Box::new);
        outcome.operation_id = Some(id);
        Ok(outcome)
    }

    /// Run a command a config reload deferred, finishing the record of the
    /// client that asked for it, if one did, the way
    /// [`Self::apply_wire_operation`] would have. The drain of the deferred
    /// queue is this record's completion point.
    pub(crate) fn apply_deferred_operation(
        &mut self,
        command: crate::engine::Command,
        operation: Option<String>,
    ) -> anyhow::Result<crate::engine::EventReaction> {
        let Some(id) = operation else {
            return self.apply(command);
        };
        let now = Instant::now();
        let before = IdSnapshot::of(self);
        self.operation_in_dispatch = Some(id.clone());
        let result = self.apply(command);
        self.operation_in_dispatch = None;
        match &result {
            Ok(reaction) => {
                self.operations
                    .note(&id, before.changes_to(&IdSnapshot::of(self)));
                let mut status = crate::wire::added_status_message(reaction)
                    .map(|message| crate::wire::WireStatus::new("info", message))
                    .or_else(|| crate::wire::wire_status_from_reaction(reaction));
                let settled = crate::wire::settled_final_from_reaction(reaction, status.as_ref());
                let running = status
                    .as_ref()
                    .filter(|s| StatusTone::from_wire(&s.tone) == StatusTone::Busy)
                    .and_then(|s| s.key.clone());
                // The client already holds the id, so a key the run mints is
                // waited on rather than taken as the id.
                self.settle_operation(&id, running, status.as_mut(), settled.as_ref(), false, now);
            }
            Err(error) => {
                self.operations
                    .finish(&id, StatusTone::Error, &format!("{error:#}"), None, now);
            }
        }
        result
    }

    /// Where a record goes once its change has answered: still running under
    /// `running` (a key minted for it alone becomes its id when `may_rekey`,
    /// any other key is waited on), ended at once by a final the change
    /// already reached (`settled`), or ended on the status it answered with,
    /// which carries the record's id as its key when it had none. Answers the
    /// record's id.
    fn settle_operation(
        &self,
        provisional: &str,
        running: Option<String>,
        status: Option<&mut crate::wire::WireStatus>,
        settled: Option<&crate::wire::WireStatus>,
        may_rekey: bool,
        now: Instant,
    ) -> String {
        match running {
            Some(key) => {
                let id = if may_rekey && crate::engine::status_op::is_minted_status_id(&key) {
                    self.operations.rekey(provisional, &key);
                    key.clone()
                } else {
                    self.operations.await_key(provisional, &key);
                    provisional.to_string()
                };
                if let Some(settled) = settled {
                    self.operations.finish_by_key(
                        &key,
                        StatusTone::from_wire(&settled.tone),
                        &settled.message,
                        settled.segments.as_deref(),
                        now,
                    );
                }
                id
            }
            None => {
                match status {
                    Some(status) => {
                        if status.key.is_none() {
                            status.key = Some(provisional.to_string());
                        }
                        self.operations.finish(
                            provisional,
                            StatusTone::from_wire(&status.tone),
                            &status.message,
                            status.segments.as_deref(),
                            now,
                        );
                    }
                    None => self
                        .operations
                        .finish(provisional, StatusTone::Info, "", None, now),
                }
                provisional.to_string()
            }
        }
    }

    /// Open a running record under `id`, with the policy `[server]` sets now.
    /// A change dispatched outside [`Self::apply_wire_operation`] (a tab or a
    /// terminal) opens its record here before it starts.
    pub fn open_operation(&self, id: &str, kind: OperationKind) {
        self.operations.open(
            id,
            kind,
            OperationPolicy::from_server(&self.config.server),
            Instant::now(),
        );
    }

    /// Add facts to the record waiting on `key`, or, with no key, to the
    /// record of the change being dispatched right now. A no-op when neither
    /// names a record, which is every change nobody asked about.
    pub fn note_operation(&self, key: Option<&str>, notes: OperationNotes) {
        if let Some(key) = key.or(self.operation_in_dispatch.as_deref()) {
            self.operations.note(key, notes);
        }
    }

    /// A launch reported back: finish every record waiting on `key` (a
    /// tab's [`launch_binding_key`], or for a failure reported by agent, its
    /// [`launch_failure_key`]). A completion point of its own, because the
    /// launch's status rides a key shared by every launch of that tab.
    ///
    /// A launch that came up succeeds. One that did not is a failure, unless
    /// the record created `tab_id` and that tab is still there (a promotion
    /// kept it, or its cleanup failed): then the create half-happened, and the
    /// record says so. A tab that is gone is no longer counted as created.
    pub fn finish_launch_operations(
        &self,
        key: &str,
        tab_id: Option<&str>,
        launched: bool,
        message: &str,
        segments: Option<&[ProseSegment]>,
    ) {
        let now = Instant::now();
        if launched {
            self.operations.finish_awaiting_as(
                key,
                OperationState::Succeeded,
                message,
                segments,
                now,
            );
            return;
        }
        if let Some(tab_id) = tab_id
            && self.owning_session_for_tab(tab_id).is_none()
        {
            self.operations.retract_created(key, tab_id);
        }
        self.operations
            .finish_by_key(key, StatusTone::Error, message, segments, now);
    }

    /// Forget the finished records past their retention. Called from the one
    /// per-tick engine call every serving surface makes, so a registry nobody
    /// reads still lets go of what it no longer has to keep.
    pub fn prune_operations(&self) {
        self.operations.prune(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: OperationPolicy = OperationPolicy {
        unknown_after: Duration::from_secs(60),
        retention: Duration::from_secs(120),
    };

    fn state_of(ops: &Operations, id: &str, now: Instant) -> OperationState {
        ops.view(id, now).expect("record present").state
    }

    #[test]
    fn a_keyed_final_finishes_the_record_waiting_on_it_with_its_sentence() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);
        assert_eq!(state_of(&ops, "op-1", t0), OperationState::Running);

        let segments = vec![ProseSegment::Text("Deleted.".to_string())];
        ops.finish_by_key("op-1", StatusTone::Info, "Deleted.", Some(&segments), t0);

        let view = ops.view("op-1", t0).unwrap();
        assert_eq!(view.state, OperationState::Succeeded);
        assert_eq!(view.message, "Deleted.");
        assert_eq!(view.segments, Some(segments));
    }

    #[test]
    fn the_final_settles_the_state_with_the_notes() {
        let refused_branch = OperationPart {
            part: PartKind::Branch,
            subject: "feat".to_string(),
            outcome: PartOutcome::Refused,
            reason: Some("checked out elsewhere".to_string()),
        };
        let removed_worktree = OperationPart {
            part: PartKind::Worktree,
            subject: "/w".to_string(),
            outcome: PartOutcome::Removed,
            reason: None,
        };
        let cases: Vec<(&str, StatusTone, OperationNotes, OperationState)> = vec![
            (
                "an info with nothing missed",
                StatusTone::Info,
                OperationNotes::default(),
                OperationState::Succeeded,
            ),
            (
                "an error with nothing done",
                StatusTone::Error,
                OperationNotes::default(),
                OperationState::Failed,
            ),
            (
                "a warning with nothing done",
                StatusTone::Warning,
                OperationNotes::default(),
                OperationState::Failed,
            ),
            (
                "a warning after something was created",
                StatusTone::Warning,
                OperationNotes {
                    created: vec!["s1".to_string()],
                    ..Default::default()
                },
                OperationState::Partial,
            ),
            (
                "an error after something was removed",
                StatusTone::Error,
                OperationNotes {
                    removed: vec!["s1".to_string()],
                    ..Default::default()
                },
                OperationState::Partial,
            ),
            (
                "a removed worktree beside a refused branch",
                StatusTone::Warning,
                OperationNotes {
                    parts: vec![removed_worktree, refused_branch.clone()],
                    ..Default::default()
                },
                OperationState::Partial,
            ),
            (
                "only a refused part",
                StatusTone::Info,
                OperationNotes {
                    parts: vec![refused_branch],
                    ..Default::default()
                },
                OperationState::Failed,
            ),
        ];
        for (name, tone, notes, expected) in cases {
            let ops = Operations::default();
            let t0 = Instant::now();
            ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);
            ops.note("op-1", notes);
            ops.finish_by_key("op-1", tone, "x", None, t0);
            assert_eq!(state_of(&ops, "op-1", t0), expected, "{name}");
        }
    }

    #[test]
    fn a_record_past_its_unknown_threshold_reads_unknown_stays_open_and_finishes_with_the_real_outcome()
     {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);

        let late = t0 + Duration::from_secs(61);
        assert_eq!(state_of(&ops, "op-1", late), OperationState::Unknown);
        // Far past the threshold AND the retention: an open record is kept.
        let much_later = t0 + Duration::from_secs(10_000);
        assert_eq!(state_of(&ops, "op-1", much_later), OperationState::Unknown);

        ops.finish_by_key("op-1", StatusTone::Error, "git refused", None, much_later);
        let view = ops.view("op-1", much_later).unwrap();
        assert_eq!(view.state, OperationState::Failed);
        assert_eq!(view.message, "git refused");
    }

    #[test]
    fn a_finished_record_is_kept_for_its_retention_and_then_forgotten() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::TabClose, POLICY, t0);
        ops.finish("op-1", StatusTone::Info, "Closed.", None, t0);

        assert!(ops.view("op-1", t0 + Duration::from_secs(119)).is_some());
        assert!(ops.view("op-1", t0 + Duration::from_secs(120)).is_none());
    }

    #[test]
    fn a_final_under_another_key_leaves_the_record_running() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);
        ops.finish_by_key("op-2", StatusTone::Info, "other", None, t0);
        assert_eq!(state_of(&ops, "op-1", t0), OperationState::Running);
    }

    #[test]
    fn a_hand_off_moves_the_wait_to_the_operation_that_carries_the_outcome() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-lookup", OperationKind::AgentCreate, POLICY, t0);
        ops.hand_off("op-lookup", "op-create");

        // The lookup's own key is cleared on a hand-off; that is not the end.
        ops.finish_by_key("op-lookup", StatusTone::Info, "", None, t0);
        assert_eq!(state_of(&ops, "op-lookup", t0), OperationState::Running);

        ops.note(
            "op-create",
            OperationNotes {
                created: vec!["s9".to_string()],
                ..Default::default()
            },
        );
        ops.finish_by_key("op-create", StatusTone::Info, "Created.", None, t0);
        let view = ops.view("op-lookup", t0).unwrap();
        assert_eq!(view.state, OperationState::Succeeded);
        assert_eq!(view.created, vec!["s9".to_string()]);
    }

    #[test]
    fn a_record_rekeyed_before_anybody_knew_its_id_waits_on_the_new_one() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-tmp", OperationKind::AgentDelete, POLICY, t0);
        ops.note(
            "op-tmp",
            OperationNotes {
                removed: vec!["s1".to_string()],
                ..Default::default()
            },
        );
        ops.rekey("op-tmp", "op-7");

        assert!(ops.view("op-tmp", t0).is_none());
        ops.finish_by_key("op-7", StatusTone::Info, "Deleted.", None, t0);
        let view = ops.view("op-7", t0).unwrap();
        assert_eq!(view.state, OperationState::Succeeded);
        assert_eq!(view.removed, vec!["s1".to_string()]);
    }

    #[test]
    fn records_waiting_on_one_shared_key_all_finish_with_its_final() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::AgentStop, POLICY, t0);
        ops.open("op-2", OperationKind::AgentStop, POLICY, t0);
        ops.await_key("op-1", "detach-agent:s1");
        ops.await_key("op-2", "detach-agent:s1");

        ops.finish_by_key("detach-agent:s1", StatusTone::Info, "Stopped.", None, t0);
        assert_eq!(ops.view("op-1", t0).unwrap().message, "Stopped.");
        assert_eq!(ops.view("op-2", t0).unwrap().message, "Stopped.");
    }

    #[test]
    fn an_explicit_completion_overrides_what_the_tone_would_say() {
        let ops = Operations::default();
        let t0 = Instant::now();
        let key = launch_binding_key("t1");
        ops.open("op-1", OperationKind::TabCreate, POLICY, t0);
        ops.note(
            "op-1",
            OperationNotes {
                created: vec!["t1".to_string()],
                ..Default::default()
            },
        );
        ops.await_key("op-1", &key);

        ops.finish_awaiting_as(&key, OperationState::Failed, "Tab launch failed", None, t0);
        assert_eq!(state_of(&ops, "op-1", t0), OperationState::Failed);
    }

    #[test]
    fn a_record_finishes_once() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::TabClose, POLICY, t0);
        ops.finish("op-1", StatusTone::Info, "first", None, t0);
        ops.finish("op-1", StatusTone::Error, "second", None, t0);
        ops.finish_by_key("op-1", StatusTone::Error, "third", None, t0);
        let view = ops.view("op-1", t0).unwrap();
        assert_eq!(view.state, OperationState::Succeeded);
        assert_eq!(view.message, "first");
    }

    mod dispatch {
        use super::*;
        use crate::engine::test_support::{sample_project, sample_session, test_engine};
        use crate::wire::WireCommand;

        fn engine_with_session() -> (crate::engine::Engine, crate::test_scratch::ScratchDir) {
            let (mut engine, tmp) = test_engine();
            engine.projects.push(sample_project("p1", "/tmp/p1"));
            let session = sample_session("s1", "p1", "feat");
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
            (engine, tmp)
        }

        #[test]
        fn a_change_that_finishes_inside_the_call_ends_its_record_on_the_final_it_answered_with() {
            let (mut engine, _tmp) = engine_with_session();

            let outcome = engine
                .apply_wire_operation(
                    WireCommand::DeleteSession {
                        session_id: "s1".to_string(),
                        delete_worktree: false,
                        delete_branch: None,
                    },
                    OperationKind::AgentDelete,
                )
                .expect("the delete dispatches");

            let id = outcome.operation_id.clone().expect("an operation id");
            let status = outcome.status.expect("the delete's final");
            assert_eq!(
                status.key.as_deref(),
                Some(id.as_str()),
                "the final carries the id"
            );
            let view = engine
                .operations
                .view(&id, Instant::now())
                .expect("a record");
            assert_eq!(view.kind, OperationKind::AgentDelete);
            assert_eq!(view.state, OperationState::Succeeded);
            assert_eq!(view.message, status.message);
            assert_eq!(view.removed, vec!["s1".to_string()]);
            assert_eq!(
                view.parts,
                vec![
                    OperationPart {
                        part: PartKind::Worktree,
                        subject: "/tmp/s1-worktree".to_string(),
                        outcome: PartOutcome::Kept,
                        reason: None,
                    },
                    OperationPart {
                        part: PartKind::Branch,
                        subject: "feat".to_string(),
                        outcome: PartOutcome::Kept,
                        reason: None,
                    },
                ]
            );

            // Removing the project while keeping worktrees names each agent's
            // worktree as kept.
            let session = sample_session("s2", "p1", "other");
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
            let outcome = engine
                .apply_wire_operation(
                    WireCommand::RemoveProject {
                        project_id: "p1".to_string(),
                    },
                    OperationKind::ProjectRemove,
                )
                .expect("the removal dispatches");
            let view = engine
                .operations
                .view(&outcome.operation_id.unwrap(), Instant::now())
                .unwrap();
            assert_eq!(view.state, OperationState::Succeeded);
            assert_eq!(
                view.parts,
                vec![OperationPart {
                    part: PartKind::Worktree,
                    subject: "/tmp/s2-worktree".to_string(),
                    outcome: PartOutcome::Kept,
                    reason: None,
                }]
            );
        }

        #[test]
        fn the_engine_forgets_finished_records_on_its_periodic_pass() {
            let (mut engine, _tmp) = engine_with_session();
            let long_ago = Instant::now()
                .checked_sub(Duration::from_secs(10))
                .expect("the clock reaches back ten seconds");
            engine
                .operations
                .open("op-old", OperationKind::TabClose, POLICY_1S, long_ago);
            engine
                .operations
                .finish("op-old", StatusTone::Info, "", None, long_ago);
            assert_eq!(engine.operations.len(), 1);

            engine.poll_pty_activity();

            assert_eq!(
                engine.operations.len(),
                0,
                "a finished record past its retention goes"
            );
        }

        const POLICY_1S: OperationPolicy = OperationPolicy {
            unknown_after: Duration::from_secs(60),
            retention: Duration::from_secs(1),
        };

        #[test]
        fn a_change_refused_outright_leaves_no_record() {
            let (mut engine, _tmp) = engine_with_session();
            let error = engine
                .apply_wire_operation(
                    WireCommand::DetachAgent {
                        session_id: "ghost".to_string(),
                        force: false,
                    },
                    OperationKind::AgentStop,
                )
                .expect_err("an unknown agent is refused");
            assert!(error.to_string().contains("unknown session"), "{error}");
            assert!(
                format!("{:?}", engine.operations).contains("Operations(0)"),
                "no record is left behind: {:?}",
                engine.operations
            );
        }

        #[test]
        fn a_worker_run_delete_is_followed_under_its_own_key_and_learns_what_the_removal_left() {
            let (mut engine, _tmp) = engine_with_session();

            let outcome = engine
                .apply_wire_operation(
                    WireCommand::DeleteSession {
                        session_id: "s1".to_string(),
                        delete_worktree: true,
                        delete_branch: Some(true),
                    },
                    OperationKind::AgentDelete,
                )
                .expect("the delete dispatches");

            let busy = outcome.status.expect("the delete's busy");
            assert_eq!(busy.tone, "busy");
            let id = outcome.operation_id.expect("an operation id");
            assert_eq!(
                busy.key.as_deref(),
                Some(id.as_str()),
                "named after the delete's op"
            );
            let running = engine.operations.view(&id, Instant::now()).unwrap();
            assert_eq!(running.state, OperationState::Running);
            assert_eq!(running.removed, vec!["s1".to_string()]);

            let _ =
                engine.drive_delete_followup(&crate::engine::EventReaction::WorktreeRemoveFailed {
                    session_id: "s1".to_string(),
                    message: "not a working tree".to_string(),
                });
            let view = engine.operations.view(&id, Instant::now()).unwrap();
            assert_eq!(
                view.parts,
                vec![
                    OperationPart {
                        part: PartKind::Worktree,
                        subject: "/tmp/s1-worktree".to_string(),
                        outcome: PartOutcome::Failed,
                        reason: Some("not a working tree".to_string()),
                    },
                    OperationPart {
                        part: PartKind::Branch,
                        subject: "feat".to_string(),
                        outcome: PartOutcome::Kept,
                        reason: None,
                    },
                ]
            );
        }
    }

    #[test]
    fn agent_delete_parts_name_each_branch_and_what_git_did() {
        use crate::engine::{RemovedBranches, WorktreeRemoval};
        use crate::git::{BranchDeletion, RemoveResult};
        let removed = |branch, initial| WorktreeRemoval::Performed {
            branches: RemovedBranches::Deleted(RemoveResult {
                branch,
                initial_branch: initial,
            }),
        };
        let part = |part, subject: &str, outcome, reason: Option<&str>| OperationPart {
            part,
            subject: subject.to_string(),
            outcome,
            reason: reason.map(str::to_string),
        };
        let cases = vec![
            (
                "the branch deleted",
                removed(BranchDeletion::Deleted, None),
                vec![
                    part(PartKind::Worktree, "/w", PartOutcome::Removed, None),
                    part(PartKind::Branch, "feat", PartOutcome::Deleted, None),
                ],
            ),
            (
                "the branch refused and the drifted birth branch already gone",
                removed(
                    BranchDeletion::Refused {
                        reason: "checked out".to_string(),
                    },
                    Some(BranchDeletion::AlreadyGone),
                ),
                vec![
                    part(PartKind::Worktree, "/w", PartOutcome::Removed, None),
                    part(
                        PartKind::Branch,
                        "feat",
                        PartOutcome::Refused,
                        Some("checked out"),
                    ),
                    part(PartKind::Branch, "born", PartOutcome::AlreadyGone, None),
                ],
            ),
            (
                "the branch kept because it was not dux's",
                WorktreeRemoval::Performed {
                    branches: RemovedBranches::Kept(crate::model::BranchKeptReason::UserDeclined),
                },
                vec![
                    part(PartKind::Worktree, "/w", PartOutcome::Removed, None),
                    part(PartKind::Branch, "feat", PartOutcome::Kept, None),
                ],
            ),
            (
                "a standalone agent's folder",
                WorktreeRemoval::NothingToRemove {
                    folder_label: "~/f".to_string(),
                },
                vec![],
            ),
        ];
        for (name, removal, expected) in cases {
            assert_eq!(
                agent_delete_parts("/w", "feat", "born", &removal),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn a_record_serializes_in_the_documented_shape() {
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);
        ops.note(
            "op-1",
            OperationNotes {
                removed: vec!["s1".to_string()],
                parts: vec![OperationPart {
                    part: PartKind::Branch,
                    subject: "feat".to_string(),
                    outcome: PartOutcome::AlreadyGone,
                    reason: None,
                }],
                ..Default::default()
            },
        );
        let json = serde_json::to_value(ops.view("op-1", t0).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "op-1",
                "kind": "agent.delete",
                "state": "running",
                "message": "",
                "created": [],
                "removed": ["s1"],
                "parts": [{"part": "branch", "subject": "feat", "outcome": "already_gone"}],
            })
        );
    }
}
