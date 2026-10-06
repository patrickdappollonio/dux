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
//! change, the same `op_id` the create routes answer with. A
//! change that runs to its end inside the call, or whose status rides a key
//! that is not unique to it (a stop's per-agent key, a tab launch's per-tab
//! key), gets a fresh id from the same minter, so the two never collide.
//!
//! # Completion points
//!
//! A record finishes in exactly one of these places:
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
//! | `POST /sessions/{id}/tabs/{tab}/stop` | inside the call |
//! | `POST` and `DELETE` on the three terminal addresses | inside the call |
//! | `PUT` and `DELETE` on `/macros/{name}` and `/global-env/{name}` | inside the call |
//! | `POST /config/reload` | its reload's owner saying how it ended (`Engine::finish_config_reload_operations`) |
//!
//! A config reload has one owner per way of serving, and each says how the
//! reload ended: the terminal UI's handlers for a reloaded config, an adopted
//! one and a refused one (plain terminal UI and background serving), and the
//! web actor's reload follow-up (`dux server` and the flip). A reload asked
//! for while another is reading waits for the follow-up that reads the file
//! after it.
//!
//! A record still running past its policy's `unknown_after` reads as
//! [`OperationState::Unknown`] but stays open, and finishes with the real
//! outcome whenever the work ends. Nothing here ever finishes a record on a
//! timer.
//!
//! # Admission
//!
//! A route opens a record for every change it covers, whether or not the
//! client asked to follow it (`Engine::apply_wire_recorded` when it did not,
//! which leaves the statuses and the answer exactly as they were).
//!
//! An open record also holds the admission keys of what its change is
//! changing ([`Operations::admit`], decided by `crate::engine::admission`),
//! so a second change to the same thing is refused with an [`InTheWay`]
//! naming this record, for exactly as long as the record runs. Finishing the
//! record is what releases them; there is no other release to forget. An
//! agent delete also keeps the agent as it looked when the delete started
//! ([`Operations::removing_agents`]), so a client's list can still show it,
//! as being removed, after it has left the workspace.

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
    #[serde(rename = "tab.stop")]
    TabStop,
    #[serde(rename = "terminal.create")]
    TerminalCreate,
    #[serde(rename = "terminal.close")]
    TerminalClose,
    #[serde(rename = "macro.set")]
    MacroSet,
    #[serde(rename = "macro.remove")]
    MacroRemove,
    #[serde(rename = "env.set")]
    EnvSet,
    #[serde(rename = "env.remove")]
    EnvRemove,
    #[serde(rename = "config.reload")]
    ConfigReload,
}

impl OperationKind {
    /// What dux is doing while a record of this kind runs, as a refusal names
    /// it: "dux is still {this}".
    pub fn running_phrase(self) -> &'static str {
        match self {
            Self::ProjectAdd => "adding a project",
            Self::ProjectRemove => "removing a project",
            Self::AgentCreate => "creating an agent",
            Self::AgentDelete => "deleting an agent",
            Self::AgentStop => "stopping an agent",
            Self::AgentStart => "starting an agent",
            Self::TabCreate => "opening a tab",
            Self::TabClose => "closing a tab",
            Self::TabStart => "starting a tab",
            Self::TabStop => "stopping a tab",
            Self::TerminalCreate => "opening a terminal",
            Self::TerminalClose => "closing a terminal",
            Self::MacroSet => "saving a macro",
            Self::MacroRemove => "removing a macro",
            Self::EnvSet => "saving a global environment variable",
            Self::EnvRemove => "removing a global environment variable",
            Self::ConfigReload => "reloading the config",
        }
    }
}

/// How a record holds an admission key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldMode {
    /// Nothing else may change the thing while this is held.
    Exclusive,
    /// Other shared holds may sit beside this one; an exclusive one may not.
    Shared,
}

/// One admission key and how it is held or wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    pub key: crate::engine::InFlightKey,
    pub mode: HoldMode,
}

impl Hold {
    pub fn exclusive(key: crate::engine::InFlightKey) -> Self {
        Self {
            key,
            mode: HoldMode::Exclusive,
        }
    }

    pub fn shared(key: crate::engine::InFlightKey) -> Self {
        Self {
            key,
            mode: HoldMode::Shared,
        }
    }

    fn conflicts_with(&self, other: &Hold) -> bool {
        self.key == other.key
            && (self.mode == HoldMode::Exclusive || other.mode == HoldMode::Exclusive)
    }
}

/// A change refused because an operation still running holds what it wants.
/// Its sentence names that operation and its id, which a client can follow at
/// `GET /api/v1/operations/{id}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InTheWay {
    pub id: String,
    pub kind: OperationKind,
}

/// The engine's refusal of a create while another create or fork is running.
/// It names no operation: a client that waits for the other one reads its id
/// from the route's answer instead.
pub const CREATE_IN_FLIGHT_REFUSAL: &str = "An agent is already being created or forked.";

/// How every [`InTheWay`] sentence starts, so a route can tell this refusal
/// from the others an engine error carries (see [`is_in_the_way`]).
const IN_THE_WAY_LEAD: &str = "Another change is still running here";

impl std::fmt::Display for InTheWay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{IN_THE_WAY_LEAD}: dux is still {} (operation {}), so it did not start this one. \
             Try again once that operation finishes.",
            self.kind.running_phrase(),
            self.id
        )
    }
}

impl std::error::Error for InTheWay {}

/// Whether an engine error's text is an [`InTheWay`] refusal: engine errors
/// reach the routes as text, and this one answers `409` rather than `400`.
pub fn is_in_the_way(message: &str) -> bool {
    message.starts_with(IN_THE_WAY_LEAD)
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

    /// The outcome these notes and a final of `tone` add up to.
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
/// finished one is kept; fixed when the record opens, so a reload affects only later ones.
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

/// The key a tab create or start waits on, finished by that tab's launch report.
/// Not a status key: every launch of a tab shares one, so it cannot name a request.
pub fn launch_binding_key(tab_id: &str) -> String {
    format!("launch:{tab_id}")
}

/// The key a failed launch of an agent's first tab moves its waiting records to:
/// that failure is reported by agent, and a promotion may move the first slot first.
pub fn launch_failure_key(session_id: &str) -> String {
    format!("launch-failed:{session_id}")
}

/// What a record waiting for a config reload waits on. Not a status key: the
/// reload's own statuses do not end it; only its owner saying how it ended does.
const RELOAD_KEY_PREFIX: &str = "reload-epoch:";

/// A reload's outcome with the listener changes that went wrong folded in:
/// a reload that applied becomes partial, and the sentence names each.
fn merge_listener_problems(
    state: OperationState,
    message: &str,
    problems: &mut Vec<String>,
) -> (OperationState, String) {
    let problems = std::mem::take(problems);
    if problems.is_empty() {
        return (state, message.to_string());
    }
    let state = match state {
        OperationState::Succeeded => OperationState::Partial,
        other => other,
    };
    let message = format!(
        "{message} The listener changes this reload asked for did not all work: {}",
        problems.join(" ")
    );
    (state, message)
}

fn reload_key(epoch: u64) -> String {
    format!("{RELOAD_KEY_PREFIX}{epoch}")
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
    /// Minted when the record opens and never changed, so a [`RecordWatch`]
    /// finds the same record after a [`Operations::rekey`] moves its id.
    token: u64,
    /// The key whose final finishes this record, while it runs.
    awaiting: Option<String>,
    notes: OperationNotes,
    end: Option<End>,
    /// The admission keys this record holds. Read only while `end` is `None`,
    /// so finishing the record is what releases them.
    holds: Vec<Hold>,
    /// The agent a delete is removing, as it looked when the delete started, so
    /// a client's list can show it as being removed after it leaves the workspace.
    removing: Option<Box<crate::viewmodel::SessionView>>,
}

impl Record {
    fn end_as(
        &mut self,
        state: OperationState,
        message: &str,
        segments: Option<&[ProseSegment]>,
        now: Instant,
    ) {
        self.end = Some(End {
            state,
            message: message.to_string(),
            segments: segments.map(<[ProseSegment]>::to_vec),
            at: now,
        });
        self.awaiting = None;
    }
}

#[derive(Default)]
struct Registry {
    records: HashMap<String, Record>,
    next_token: u64,
    /// The reload whose barrier is open, or the next to open; a record asked for
    /// while one is already reading follows the epoch after it.
    reload_epoch: u64,
    /// The epochs whose barrier has closed and whose owner has not yet said
    /// how it ended, oldest first.
    reloads_closed: std::collections::VecDeque<u64>,
    /// Listener changes a reload set off that have not ended.
    listener_pending: u32,
    /// How listener changes that ended went wrong, until a reload's outcome
    /// takes them.
    listener_problems: Vec<String>,
    /// Reloads whose owner has spoken but whose listener changes have not all
    /// ended: their epoch, state and sentence.
    held_reloads: Vec<(u64, OperationState, String)>,
    /// The status key of the create holding the one-create-at-a-time guard.
    create_guard: Option<String>,
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

/// One record, followed by what it was opened as rather than by its id,
/// which a rekey may change. See [`Operations::watch`].
#[derive(Clone)]
pub struct RecordWatch {
    operations: Operations,
    token: u64,
}

impl std::fmt::Debug for RecordWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RecordWatch").field(&self.token).finish()
    }
}

impl RecordWatch {
    /// Whether the record is still open: running, or unknown with the work not
    /// yet ended. A discarded or pruned record is not.
    pub fn is_open(&self) -> bool {
        self.operations.with(|registry| {
            registry
                .records
                .values()
                .any(|record| record.token == self.token && record.end.is_none())
        })
    }
}

/// The registry. Every clone is the same registry. A poisoned lock is
/// recovered, so a panic elsewhere never takes a client's answer down with it.
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
            registry.next_token += 1;
            let token = registry.next_token;
            registry.records.insert(
                id.to_string(),
                Record {
                    kind,
                    policy,
                    started: now,
                    token,
                    awaiting: Some(id.to_string()),
                    notes: OperationNotes::default(),
                    end: None,
                    holds: Vec::new(),
                    removing: None,
                },
            );
        });
    }

    /// Admits a change that wants `wants` and, under the same lock, adds `takes`
    /// to `record`'s holds; with no record (`None`) nothing is held past the call.
    ///
    /// # Errors
    ///
    /// [`InTheWay`] naming the earliest-started open record that holds any of
    /// `wants` in a conflicting mode.
    pub fn admit(
        &self,
        record: Option<&str>,
        wants: &[Hold],
        takes: &[Hold],
    ) -> Result<(), InTheWay> {
        self.with(|registry| {
            let blocker = registry
                .records
                .iter()
                .filter(|(id, held)| held.end.is_none() && Some(id.as_str()) != record)
                .filter(|(_, held)| {
                    held.holds
                        .iter()
                        .any(|hold| wants.iter().any(|want| want.conflicts_with(hold)))
                })
                .min_by_key(|(_, held)| held.started);
            if let Some((id, held)) = blocker {
                return Err(InTheWay {
                    id: id.clone(),
                    kind: held.kind,
                });
            }
            if let Some(own) = record.and_then(|id| registry.records.get_mut(id))
                && own.end.is_none()
            {
                own.holds.extend_from_slice(takes);
            }
            Ok(())
        })
    }

    /// Add `hold` to the records waiting on `key` or whose id it is: a change
    /// that made a thing holds it from then on.
    pub fn hold(&self, key: &str, hold: Hold) {
        self.with(|registry| {
            for (id, record) in registry.records.iter_mut() {
                if record.end.is_none()
                    && (id.as_str() == key || record.awaiting.as_deref() == Some(key))
                    && !record.holds.contains(&hold)
                {
                    record.holds.push(hold.clone());
                }
            }
        });
    }

    /// Keep `view` on record `id` as the agent it is removing.
    pub fn set_removing(&self, id: &str, view: crate::viewmodel::SessionView) {
        self.with(|registry| {
            if let Some(record) = registry.records.get_mut(id) {
                record.removing = Some(Box::new(view));
            }
        });
    }

    /// The agents an open record is still removing, oldest change first.
    pub fn removing_agents(&self) -> Vec<crate::viewmodel::SessionView> {
        self.with(|registry| {
            let mut open: Vec<&Record> = registry
                .records
                .values()
                .filter(|record| record.end.is_none() && record.removing.is_some())
                .collect();
            open.sort_by_key(|record| record.started);
            open.into_iter()
                .filter_map(|record| record.removing.as_deref().cloned())
                .collect()
        })
    }

    /// The open record holding `key` in any mode, if one does.
    pub fn holder_of(&self, key: &crate::engine::InFlightKey) -> Option<InTheWay> {
        self.with(|registry| {
            registry
                .records
                .iter()
                .filter(|(_, record)| {
                    record.end.is_none() && record.holds.iter().any(|hold| &hold.key == key)
                })
                .min_by_key(|(_, record)| record.started)
                .map(|(id, record)| InTheWay {
                    id: id.clone(),
                    kind: record.kind,
                })
        })
    }

    /// Note that the create the engine started under the status key `key` holds
    /// the one-create-at-a-time guard, until [`Self::clear_create_guard`].
    pub fn set_create_guard(&self, key: &str) {
        self.with(|registry| registry.create_guard = Some(key.to_string()));
    }

    /// The guard's create ended.
    pub fn clear_create_guard(&self) {
        self.with(|registry| registry.create_guard = None);
    }

    /// The id of the open record following the create that holds the guard,
    /// or `None` when nothing holds it or no record follows that create (one a
    /// terminal UI started has none).
    pub fn create_guard_record(&self) -> Option<String> {
        self.with(|registry| {
            let key = registry.create_guard.as_deref()?;
            registry
                .records
                .iter()
                .filter(|(id, record)| {
                    record.end.is_none()
                        && (id.as_str() == key || record.awaiting.as_deref() == Some(key))
                })
                .min_by_key(|(_, record)| record.started)
                .map(|(id, _)| id.clone())
        })
    }

    /// A watch on the record `id` that keeps finding it after a rekey, or
    /// `None` when there is no such record.
    pub fn watch(&self, id: &str) -> Option<RecordWatch> {
        self.with(|registry| registry.records.get(id).map(|record| record.token))
            .map(|token| RecordWatch {
                operations: self.clone(),
                token,
            })
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

    /// Make the running record `id` wait for the next reload to read the file:
    /// the open barrier's, or with `queued` (one is already reading) its follow-up.
    pub fn await_reload(&self, id: &str, queued: bool) {
        self.with(|registry| {
            let key = reload_key(registry.reload_epoch + u64::from(queued));
            if let Some(record) = registry.records.get_mut(id)
                && record.end.is_none()
            {
                record.awaiting = Some(key);
            }
        });
    }

    /// Whether the running record `id` waits for a config reload.
    pub fn awaits_reload(&self, id: &str) -> bool {
        self.with(|registry| {
            registry
                .records
                .get(id)
                .and_then(|record| record.awaiting.as_deref())
                .is_some_and(|key| key.starts_with(RELOAD_KEY_PREFIX))
        })
    }

    /// The reload whose barrier was open has closed; its owner reports how it
    /// ended through [`Self::finish_reload`].
    pub fn reload_closed(&self) {
        self.with(|registry| {
            let closed = registry.reload_epoch;
            registry.reloads_closed.push_back(closed);
            registry.reload_epoch += 1;
        });
    }

    /// The owner of the oldest closed reload says how it ended. Its records finish
    /// now, or once the listener changes it set off have ended.
    pub fn finish_reload(&self, state: OperationState, message: &str, now: Instant) {
        let (closed, waiting) = self.with(|registry| {
            let closed = registry.reloads_closed.pop_front();
            let waiting = registry.listener_pending > 0;
            if let (Some(epoch), true) = (closed, waiting) {
                registry
                    .held_reloads
                    .push((epoch, state, message.to_string()));
            }
            (closed, waiting)
        });
        if let Some(epoch) = closed
            && !waiting
        {
            let (state, message) = self.with(|registry| {
                merge_listener_problems(state, message, &mut registry.listener_problems)
            });
            self.finish_awaiting_as(&reload_key(epoch), state, &message, None, now);
        }
    }

    /// The reload being applied started a listener change that ends later; pair
    /// it with [`Self::listener_change_done`].
    pub fn listener_change_started(&self) {
        self.with(|registry| registry.listener_pending += 1);
    }

    /// A listener change the reload asked for was refused before it started.
    pub fn listener_change_failed(&self, problem: String) {
        self.with(|registry| registry.listener_problems.push(problem));
    }

    /// A listener change ended, with `problem` saying how when it did not work.
    /// When the last one ends, the reloads that waited for them finish.
    pub fn listener_change_done(&self, problem: Option<String>, now: Instant) {
        let released = self.with(|registry| {
            registry.listener_pending = registry.listener_pending.saturating_sub(1);
            registry.listener_problems.extend(problem);
            if registry.listener_pending > 0 || registry.held_reloads.is_empty() {
                return Vec::new();
            }
            let held = std::mem::take(&mut registry.held_reloads);
            let problems = std::mem::take(&mut registry.listener_problems);
            held.into_iter()
                .map(|(epoch, state, message)| {
                    let mut own = problems.clone();
                    (epoch, merge_listener_problems(state, &message, &mut own))
                })
                .collect()
        });
        for (epoch, (state, message)) in released {
            self.finish_awaiting_as(&reload_key(epoch), state, &message, None, now);
        }
    }

    /// The reload about to read the file could not start: its waiting records
    /// fail with `message`, and a later reload never completes them.
    pub fn fail_reload(&self, message: &str, now: Instant) {
        let epoch = self.with(|registry| {
            let epoch = registry.reload_epoch;
            registry.reload_epoch += 1;
            epoch
        });
        self.finish_awaiting_as(
            &reload_key(epoch),
            OperationState::Failed,
            message,
            None,
            now,
        );
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
                record.end_as(state, message, segments, now);
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
                record.end_as(state, message, segments, now);
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
                record.end_as(state, message, segments, now);
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

    /// [`Self::view`] without pruning first, so a snapshot taken where the change
    /// ended holds whatever the retention is.
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

/// Whether settling may adopt a minted key as the record's id (only before
/// anybody was told the id), and stamp the id on a keyless final.
#[derive(Clone, Copy)]
struct Settle {
    may_rekey: bool,
    stamp_key: bool,
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
    /// Dispatch `command` as a change a client will follow, opening its record
    /// before any of its statuses is raised; the outcome's `operation_id` names it.
    ///
    /// # Errors
    ///
    /// The command's own refusal, which leaves no record.
    pub fn apply_wire_operation(
        &mut self,
        command: crate::wire::WireCommand,
        kind: OperationKind,
    ) -> anyhow::Result<crate::wire::WireCommandOutcome> {
        self.apply_wire_followed(command, kind, true)
    }

    /// [`Self::apply_wire_operation`] for a change nobody asked to follow: the
    /// record still holds what it changes, and its statuses are left untouched.
    pub fn apply_wire_recorded(
        &mut self,
        command: crate::wire::WireCommand,
        kind: OperationKind,
    ) -> anyhow::Result<crate::wire::WireCommandOutcome> {
        self.apply_wire_followed(command, kind, false)
    }

    fn apply_wire_followed(
        &mut self,
        command: crate::wire::WireCommand,
        kind: OperationKind,
        stamp_key: bool,
    ) -> anyhow::Result<crate::wire::WireCommandOutcome> {
        let now = Instant::now();
        let provisional = mint_operation_id();
        self.open_operation(&provisional, kind);
        if let crate::wire::WireCommand::DeleteSession { session_id, .. } = &command
            && let Some(view) = self.session_view(session_id)
        {
            self.operations.set_removing(&provisional, view);
        }
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
        let id = if deferred || self.operations.awaits_reload(&provisional) {
            // Deferred behind a reload, or a reload itself: nothing has happened
            // yet, so the record waits for the drain or the reload's owner.
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
                Settle {
                    may_rekey: true,
                    stamp_key,
                },
                now,
            )
        };
        outcome.operation = self.operations.peek(&id, now).map(Box::new);
        outcome.operation_id = Some(id);
        Ok(outcome)
    }

    /// Run a command a config reload deferred, settling the record of the client
    /// that asked for it the way [`Self::apply_wire_operation`] would have.
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
        // Held back again behind a reload this drain started: nothing has
        // happened yet, so the record waits for the drain that runs it.
        if self
            .deferred_operations
            .iter()
            .any(|op| op.as_deref() == Some(id.as_str()))
        {
            return result;
        }
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
                self.settle_operation(
                    &id,
                    running,
                    status.as_mut(),
                    settled.as_ref(),
                    Settle {
                        may_rekey: false,
                        stamp_key: true,
                    },
                    now,
                );
            }
            Err(error) => {
                self.operations
                    .finish(&id, StatusTone::Error, &format!("{error:#}"), None, now);
            }
        }
        result
    }

    /// Leave the record waiting on the key its change still runs under, or end it
    /// on the final it already reached; answers the record's id.
    fn settle_operation(
        &self,
        provisional: &str,
        running: Option<String>,
        status: Option<&mut crate::wire::WireStatus>,
        settled: Option<&crate::wire::WireStatus>,
        Settle {
            may_rekey,
            stamp_key,
        }: Settle,
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
                        if stamp_key && status.key.is_none() {
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

    /// Open a running record under `id`, with the policy `[server]` sets now, for
    /// a change dispatched outside [`Self::apply_wire_operation`].
    pub fn open_operation(&self, id: &str, kind: OperationKind) {
        self.operations.open(
            id,
            kind,
            OperationPolicy::from_server(&self.config.server),
            Instant::now(),
        );
    }

    /// Add facts to the record waiting on `key`, or, with no key, to the record
    /// being dispatched right now; a no-op when neither names a record.
    pub fn note_operation(&self, key: Option<&str>, notes: OperationNotes) {
        if let Some(key) = key.or(self.operation_in_dispatch.as_deref()) {
            self.operations.note(key, notes);
        }
    }

    /// A launch reported back: finish the records waiting on `key`. A failed
    /// launch is partial, not failed, for a record whose created tab still exists.
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

    /// A config reload ended as `outcome` says: finish the records of every client
    /// that asked for it. Called once per reload by that serving mode's reload owner.
    pub fn finish_config_reload_operations(
        &self,
        outcome: &crate::config_reload_status::ConfigReloadOutcome,
    ) {
        let (state, message) = outcome.record();
        self.operations
            .finish_reload(state, &message, Instant::now());
    }

    /// Forget the finished records past their retention, so a registry nobody
    /// reads still lets go of them.
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

    /// A reload that set a listener change going is not over until the change
    /// is: a failure of it makes the outcome partial and names it, whether it
    /// lands after the owner said the reload applied or before.
    #[test]
    fn a_reload_waits_for_the_listener_changes_it_set_off() {
        let t0 = Instant::now();
        let open = || {
            let ops = Operations::default();
            ops.open("r", OperationKind::ConfigReload, POLICY, t0);
            ops.await_reload("r", false);
            ops.reload_closed();
            ops
        };

        // Still running after the owner's word, finished by the change's.
        let ops = open();
        ops.listener_change_started();
        ops.finish_reload(OperationState::Succeeded, "Reloaded.", t0);
        assert_eq!(state_of(&ops, "r", t0), OperationState::Running);
        ops.listener_change_done(None, t0);
        let done = ops.view("r", t0).unwrap();
        assert_eq!(done.state, OperationState::Succeeded);
        assert_eq!(done.message, "Reloaded.");

        // A failed change makes it partial and names the failure.
        let ops = open();
        ops.listener_change_started();
        ops.finish_reload(OperationState::Succeeded, "Reloaded.", t0);
        ops.listener_change_done(Some("the port is taken.".to_string()), t0);
        let done = ops.view("r", t0).unwrap();
        assert_eq!(done.state, OperationState::Partial);
        assert_eq!(
            done.message,
            "Reloaded. The listener changes this reload asked for did not all work: the port is taken."
        );

        // Answered before the owner spoke, and refused outright.
        let ops = open();
        ops.listener_change_started();
        ops.listener_change_done(Some("refused.".to_string()), t0);
        assert_eq!(state_of(&ops, "r", t0), OperationState::Running);
        ops.finish_reload(OperationState::Succeeded, "Reloaded.", t0);
        assert_eq!(state_of(&ops, "r", t0), OperationState::Partial);
        let ops = open();
        ops.listener_change_failed("not asked.".to_string());
        ops.finish_reload(OperationState::Succeeded, "Reloaded.", t0);
        assert_eq!(state_of(&ops, "r", t0), OperationState::Partial);

        // A reload that failed stays failed.
        let ops = open();
        ops.finish_reload(OperationState::Failed, "No.", t0);
        assert_eq!(state_of(&ops, "r", t0), OperationState::Failed);
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
    fn a_held_key_refuses_only_the_changes_it_conflicts_with() {
        use crate::engine::InFlightKey;
        let agent = |id: &str| InFlightKey::Agent(id.to_string());
        let project = |id: &str| InFlightKey::Project(id.to_string());
        let cases: Vec<(&str, Hold, Hold, bool)> = vec![
            (
                "the same agent, both exclusive",
                Hold::exclusive(agent("s1")),
                Hold::exclusive(agent("s1")),
                true,
            ),
            (
                "a change inside an agent another change holds",
                Hold::exclusive(agent("s1")),
                Hold::shared(agent("s1")),
                true,
            ),
            (
                "another agent",
                Hold::exclusive(agent("s1")),
                Hold::exclusive(agent("s2")),
                false,
            ),
            (
                "two creates in one project",
                Hold::shared(project("p1")),
                Hold::shared(project("p1")),
                false,
            ),
            (
                "a project removal while a create holds the project",
                Hold::shared(project("p1")),
                Hold::exclusive(project("p1")),
                true,
            ),
            (
                "the macro list",
                Hold::exclusive(InFlightKey::MacroList),
                Hold::exclusive(InFlightKey::MacroList),
                true,
            ),
            (
                "the macro list beside the environment",
                Hold::exclusive(InFlightKey::MacroList),
                Hold::exclusive(InFlightKey::GlobalEnv),
                false,
            ),
        ];
        for (name, held, wanted, refused) in cases {
            let ops = Operations::default();
            let t0 = Instant::now();
            ops.open("op-1", OperationKind::AgentDelete, POLICY, t0);
            ops.admit(Some("op-1"), &[], std::slice::from_ref(&held))
                .expect("nothing else is running");
            let answer = ops.admit(None, std::slice::from_ref(&wanted), &[]);
            assert_eq!(answer.is_err(), refused, "{name}");
        }
    }

    #[test]
    fn the_create_guard_names_the_record_that_owns_it_not_the_oldest_create() {
        let ops = Operations::default();
        let t0 = Instant::now();
        assert_eq!(ops.create_guard_record(), None);
        // An older create-kind record that does not own the guard.
        ops.open("op-older", OperationKind::AgentCreate, POLICY, t0);
        ops.open(
            "op-lookup",
            OperationKind::AgentCreate,
            POLICY,
            t0 + std::time::Duration::from_secs(1),
        );
        ops.hand_off("op-lookup", "op-create");
        ops.set_create_guard("op-create");
        // The record waiting on the guard's operation is the one named.
        assert_eq!(ops.create_guard_record().as_deref(), Some("op-lookup"));

        // An operation no record follows names nothing.
        ops.set_create_guard("op-tui");
        assert_eq!(ops.create_guard_record(), None);

        ops.set_create_guard("op-create");
        ops.clear_create_guard();
        assert_eq!(ops.create_guard_record(), None);
    }

    #[test]
    fn a_refusal_names_the_operation_in_the_way_until_it_finishes() {
        use crate::engine::InFlightKey;
        let ops = Operations::default();
        let t0 = Instant::now();
        let key = InFlightKey::Agent("s1".to_string());
        ops.open("op-7", OperationKind::AgentDelete, POLICY, t0);
        ops.admit(Some("op-7"), &[], &[Hold::exclusive(key.clone())])
            .unwrap();

        // A second change wanting the agent, from a record of its own.
        ops.open("op-8", OperationKind::AgentStop, POLICY, t0);
        let refusal = ops
            .admit(Some("op-8"), &[Hold::exclusive(key.clone())], &[])
            .expect_err("the delete holds the agent");
        assert_eq!(refusal.id, "op-7");
        let sentence = refusal.to_string();
        assert!(sentence.contains("op-7"), "{sentence}");
        assert!(sentence.contains("deleting an agent"), "{sentence}");

        ops.finish("op-7", StatusTone::Info, "Deleted.", None, t0);
        assert!(
            ops.admit(None, &[Hold::exclusive(key)], &[]).is_ok(),
            "a finished record holds nothing"
        );
    }

    #[test]
    fn a_hold_given_by_key_reaches_the_record_waiting_on_it() {
        use crate::engine::InFlightKey;
        let ops = Operations::default();
        let t0 = Instant::now();
        ops.open("op-lookup", OperationKind::AgentCreate, POLICY, t0);
        ops.hand_off("op-lookup", "op-create");

        ops.hold(
            "op-create",
            Hold::exclusive(InFlightKey::Agent("s9".to_string())),
        );

        let refusal = ops
            .admit(
                None,
                &[Hold::exclusive(InFlightKey::Agent("s9".to_string()))],
                &[],
            )
            .expect_err("the create holds the agent it made");
        assert_eq!(refusal.id, "op-lookup");
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
            // Gone from the agents at once, and listed as being removed for as
            // long as the record runs.
            assert!(engine.sessions.iter().all(|s| s.id != "s1"));
            let removing: Vec<String> = engine
                .operations
                .removing_agents()
                .into_iter()
                .map(|view| view.id)
                .collect();
            assert_eq!(removing, vec!["s1".to_string()]);

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

            engine.operations.finish_by_key(
                &id,
                StatusTone::Error,
                "not a working tree",
                None,
                Instant::now(),
            );
            assert!(
                engine.operations.removing_agents().is_empty(),
                "a finished delete is no longer removing anything"
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
