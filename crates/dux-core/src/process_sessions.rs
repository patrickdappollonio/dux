//! The processes dux started for an agent, followed by the SESSION each of them
//! was started in, and taken down as a whole before that agent's worktree is
//! removed.
//!
//! Every PTY dux spawns is a session leader (portable-pty calls `setsid` in the
//! child), and so is a startup command. A session outlives its leader: a
//! background job, a `nohup`ed or disowned command, or a dev server a CLI left
//! running keeps the session id of the PTY it was started in after the shell or
//! the CLI has exited. None of the PTY's process-group signals reach a job in a
//! process group of its own, and a process that called `setsid` itself is in
//! no group or session of the PTY's at all, so only its parentage ties it to
//! the agent, and only while its parent is alive. This module answers "which of
//! those are still running" and, when a worktree is about to be removed, ends
//! them: SIGTERM and SIGHUP, then SIGKILL after the grace, then a bounded wait
//! for the kill to land.
//!
//! What it cannot see is said out loud by the caller instead: a daemon that
//! left both the session and the process tree before it was looked for.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a purge waits for a SIGKILL to land once it has been sent, on top
/// of the configured grace. A SIGKILLed process is gone as soon as the kernel
/// schedules it, so this only bounds the pathological case (a process stuck in
/// uninterruptible I/O) rather than an ordinary shutdown. Not a user setting:
/// the user's knob is the grace, and this is the floor under the step after it.
pub const KILL_SETTLE: Duration = Duration::from_secs(3);

/// How often a purge looks at the process table while it waits.
const PURGE_POLL: Duration = Duration::from_millis(50);

/// Slack allowed between dux recording a spawn and the kernel's own start time
/// for the same process, which are read from two clocks with one-second
/// resolution on the kernel's side.
const START_TIME_SLACK_SECS: u64 = 2;

/// A session dux created: a PTY's child or a startup command, both started
/// with `setsid`, so the session id is the leader's pid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessSession {
    pub sid: u32,
    /// When dux spawned the leader, in seconds since the epoch. Tells this
    /// session apart from a later one that reused the same pid: nothing that
    /// started before it can belong to it.
    pub started_at_secs: u64,
    /// The machine boot the session was recorded in ([`current_boot`]). A
    /// session recorded in an earlier boot is void: every pid has been handed
    /// out afresh since, so its number names nothing of dux's.
    pub boot: u64,
}

impl ProcessSession {
    /// A session whose leader was just spawned with pid `sid`.
    pub fn started_now(sid: u32) -> Self {
        Self {
            sid,
            started_at_secs: epoch_secs(SystemTime::now()),
            boot: current_boot(),
        }
    }

    /// Whether this session was recorded in the boot the machine is in now.
    pub fn is_this_boot(&self) -> bool {
        self.boot == current_boot()
    }
}

/// An identity for the running boot of this machine: a hash of
/// `/proc/sys/kernel/random/boot_id` on Linux, and the boot time
/// (`kern.boottime`, read through sysinfo) elsewhere. Read once per process.
pub fn current_boot() -> u64 {
    static BOOT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        let id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|text| text.trim().to_string())
            .ok()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| sysinfo::System::boot_time().to_string());
        // FNV-1a: stable across runs and builds, unlike the std hasher.
        id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// One process, identified across two looks at the table: a pid alone can be
/// reused, a pid with its start time cannot (in practice).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
}

/// One row of the process table, as much of it as these questions need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// `None` when the process exited between the listing and the question.
    pub sid: Option<u32>,
    /// Seconds since the epoch.
    pub start_time: u64,
    pub name: String,
    /// A zombie has exited: it holds no files and writes nothing.
    pub exited: bool,
}

impl ProcRow {
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            start_time: self.start_time,
        }
    }
}

/// Every live process that belongs to one of `sessions`, or was `known` from
/// an earlier look, together with all of their live descendants.
///
/// Pure, so the rules are pinned by tests against hand-built tables:
///
/// - A session is skipped when its id has been REUSED: a live process with
///   that pid leads a session of its own and started well after dux spawned the
///   original. Its members belong to somebody else. (Linux never hands out a
///   pid that still names a live session or group, so a session with members
///   and no leader is still the original one.)
/// - Descendants are followed through the parent links, which is what reaches
///   a child that called `setsid` while its parent is still around.
/// - Exited (zombie) processes are left out, and so is `self_pid`, dux itself.
pub fn members(
    table: &[ProcRow],
    sessions: &[ProcessSession],
    known: &[ProcessIdentity],
    self_pid: u32,
) -> Vec<ProcRow> {
    let by_pid: HashMap<u32, &ProcRow> = table.iter().map(|row| (row.pid, row)).collect();
    // THE membership rule, the same in this run and at the next start. A
    // session counts by its number only while its recorded leader is alive
    // with the recorded identity (its pid, and a start time matching when dux
    // spawned it). Once the leader has gone the number proves nothing (an
    // unrelated program may hold it by now), so only the processes recorded
    // by identity count: the members dux saw still running the moment it saw
    // the leader exit, or the moment a delete looked. A session from another
    // boot is void altogether.
    let led: HashMap<u32, u64> = sessions
        .iter()
        .filter(|session| session.is_this_boot())
        .filter(|session| {
            by_pid.get(&session.sid).is_some_and(|leader| {
                !leader.exited
                    && leader.sid == Some(leader.pid)
                    && leader.start_time + START_TIME_SLACK_SECS >= session.started_at_secs
                    && leader.start_time <= session.started_at_secs + START_TIME_SLACK_SECS
            })
        })
        .map(|session| {
            (
                session.sid,
                session
                    .started_at_secs
                    .saturating_sub(START_TIME_SLACK_SECS),
            )
        })
        .collect();
    let known: HashSet<ProcessIdentity> = known.iter().copied().collect();
    let eligible = |row: &ProcRow| !row.exited && row.pid != self_pid && row.pid > 1;

    let mut children: HashMap<u32, Vec<&ProcRow>> = HashMap::new();
    for row in table {
        if let Some(ppid) = row.ppid {
            children.entry(ppid).or_default().push(row);
        }
    }

    let mut found: Vec<ProcRow> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut queue: Vec<&ProcRow> = table
        .iter()
        .filter(|row| {
            row.sid
                .and_then(|sid| led.get(&sid))
                .is_some_and(|earliest| row.start_time >= *earliest)
                || known.contains(&row.identity())
        })
        .collect();
    while let Some(row) = queue.pop() {
        if !seen.insert(row.pid) {
            continue;
        }
        if eligible(row) {
            found.push(row.clone());
        }
        // Descendants by parent chain, each started no earlier than its
        // parent: a pid reused under a reparented child would have started
        // before nothing of ours. A zombie's children are still followed:
        // until they are reparented the link is the only one there is.
        if let Some(kids) = children.get(&row.pid) {
            queue.extend(
                kids.iter()
                    .copied()
                    .filter(|kid| kid.start_time >= row.start_time),
            );
        }
    }
    found.sort_by_key(|row| row.pid);
    found
}

/// The processes still running in `session` the moment its leader was seen
/// to exit, by number (the leader being gone, nothing else can be in the
/// session yet: Linux and macOS never hand out a number that still names a
/// live session), each started no earlier than the session. Recorded as the
/// session's evidence from then on. Blocking: a worker thread's call.
pub fn survivors_at_leader_exit(session: ProcessSession) -> Vec<ProcessIdentity> {
    if !session.is_this_boot() {
        return Vec::new();
    }
    let earliest = session
        .started_at_secs
        .saturating_sub(START_TIME_SLACK_SECS);
    read_process_table()
        .iter()
        .filter(|row| {
            !row.exited
                && row.sid == Some(session.sid)
                && row.start_time >= earliest
                && row.pid != std::process::id()
        })
        .map(ProcRow::identity)
        .collect()
}

/// The process table, read off the platform (`/proc` on Linux, libproc on
/// macOS) through `sysinfo`. Blocking file and syscall work: call it from a
/// worker thread, never the UI thread.
pub fn read_process_table() -> Vec<ProcRow> {
    use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes()
        .iter()
        .filter(|(_, process)| process.thread_kind().is_none())
        .map(|(pid, process)| ProcRow {
            pid: pid.as_u32(),
            ppid: process.parent().map(|parent| parent.as_u32()),
            sid: process.session_id().map(|sid| sid.as_u32()),
            start_time: process.start_time(),
            name: process.name().to_string_lossy().into_owned(),
            exited: matches!(
                process.status(),
                ProcessStatus::Zombie | ProcessStatus::Dead
            ),
        })
        .collect()
}

/// The live members of `sessions` (and their descendants) right now, as
/// identities, so a later look can still find a member whose parent link has
/// since been cut. Blocking: a worker thread's call.
pub fn snapshot(sessions: &[ProcessSession]) -> Vec<ProcessIdentity> {
    members(&read_process_table(), sessions, &[], std::process::id())
        .iter()
        .map(ProcRow::identity)
        .collect()
}

/// What a purge has to work with. The real one reads the platform's process
/// table and sends real signals; tests substitute a scripted one.
pub trait ProcessOps {
    fn table(&mut self) -> Vec<ProcRow>;
    fn signal(&mut self, pid: u32, signal: rustix::process::Signal);
    fn now(&self) -> Instant;
    fn sleep(&mut self, duration: Duration);
}

/// The platform's process table and signals.
pub struct SystemProcesses;

impl ProcessOps for SystemProcesses {
    fn table(&mut self) -> Vec<ProcRow> {
        read_process_table()
    }

    fn signal(&mut self, pid: u32, signal: rustix::process::Signal) {
        if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
            // ESRCH (already gone) is the ordinary race with a process exiting
            // on its own; nothing else is actionable here either, because the
            // next look at the table says whether it went.
            let _ = rustix::process::kill_process(pid, signal);
        }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// How a purge ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    /// Nothing is left. `stopped` counts the processes it had to ask.
    Clean { stopped: usize },
    /// These were still running after SIGKILL and the settle wait.
    Survivors(Vec<ProcRow>),
}

/// End every process in `sessions` (and in `known`, and below either): ask
/// them with SIGTERM and SIGHUP, SIGKILL whatever is left once `grace` has
/// passed, and wait up to [`KILL_SETTLE`] more for the kill to land.
///
/// Bounded by `grace + KILL_SETTLE` whatever the processes do. Blocking: a
/// worker thread's call.
pub fn purge(
    ops: &mut dyn ProcessOps,
    sessions: &[ProcessSession],
    known: &[ProcessIdentity],
    grace: Duration,
) -> PurgeOutcome {
    let self_pid = std::process::id();
    let start = ops.now();
    let kill_at = start + grace;
    let give_up_at = kill_at + KILL_SETTLE;
    let mut asked: HashSet<ProcessIdentity> = HashSet::new();
    loop {
        let now = ops.now();
        let alive = members(&ops.table(), sessions, known, self_pid);
        if alive.is_empty() {
            return PurgeOutcome::Clean {
                stopped: asked.len(),
            };
        }
        if now >= give_up_at {
            return PurgeOutcome::Survivors(alive);
        }
        for row in &alive {
            if asked.insert(row.identity()) {
                ops.signal(row.pid, rustix::process::Signal::TERM);
                ops.signal(row.pid, rustix::process::Signal::HUP);
            }
            if now >= kill_at {
                ops.signal(row.pid, rustix::process::Signal::KILL);
            }
        }
        ops.sleep(PURGE_POLL);
    }
}

/// One line per survivor for a status or a log: `name (pid N)`.
pub fn describe(rows: &[ProcRow]) -> String {
    rows.iter()
        .map(|row| format!("{} (pid {})", row.name, row.pid))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The sessions every agent has started, kept for as long as the agent exists,
/// so a worktree removal can end what an earlier run of a tab, a terminal that
/// was already closed, or a startup command left behind. Shared, because the
/// startup command registers from its own worker thread.
///
/// Runtime only: a session is meaningless after dux restarts, since nothing
/// guarantees the pid still names it.
#[derive(Clone, Default)]
pub struct AgentProcessRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Default)]
struct RegistryInner {
    /// Per session, the members dux saw still running when it saw the
    /// session's leader exit (or when a delete looked). The only evidence a
    /// leaderless session's members are dux's.
    survivors: HashMap<ProcessSession, Vec<ProcessIdentity>>,
    /// Each owner's sessions, with the folder each was started in.
    sessions: HashMap<String, Vec<(ProcessSession, std::path::PathBuf)>>,
    /// Sessions of agents already deleted, kept (bounded) so a removal of the
    /// folder they ran in still ends what they left behind: a "keep the
    /// worktree" delete forgets the agent while its CLI is still stopping.
    retired: std::collections::VecDeque<(ProcessSession, std::path::PathBuf)>,
    /// Agents whose startup command is running right now, and whether the
    /// agent has been deleted under it.
    startup_runs: HashMap<String, bool>,
    /// Owners that are standalone agents. Their sessions keep that kind for
    /// life: no removal ever ends them, and while they run they occupy the
    /// folder they are in.
    standalone_owners: HashSet<String>,
    /// Retired sessions whose owner was a standalone agent.
    retired_standalone: HashSet<ProcessSession>,
    /// The session database, once a removal has been recorded in it: a
    /// survivor recorded later for a folder with a pending removal is written
    /// into that removal's row, so a later start ends the same set.
    pending_store: Option<std::path::PathBuf>,
}

/// The registry key for a PTY that belongs to no agent (a project or a
/// standalone terminal). It is never purged; it is registered so a deleted
/// agent's old session number, reused for one of these, is recognised as
/// somebody else's.
pub const UNOWNED_PTYS: &str = "\u{0}unowned";

/// How many sessions of deleted agents are remembered, newest kept.
const RETIRED_SESSIONS: usize = 1024;

/// How many sessions are remembered per agent. An agent that has opened and
/// closed more terminals than this over its life keeps the newest; the purge
/// still follows every live process's own session, so this only drops the
/// oldest leftovers from the list of places to look.
const SESSIONS_PER_AGENT: usize = 256;

impl AgentProcessRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Remember that `session` was started for agent `agent_id` in `folder`.
    pub fn register(&self, agent_id: &str, session: ProcessSession, folder: &std::path::Path) {
        let folder = crate::worktree_ops::path_key(folder);
        let mut inner = self.lock();
        // The kernel handed this number to a newer session: whatever was
        // recorded under it before is gone, under every key.
        let older = |(known, _): &(ProcessSession, std::path::PathBuf)| {
            known.sid == session.sid && known.started_at_secs < session.started_at_secs
        };
        for list in inner.sessions.values_mut() {
            list.retain(|entry| !older(entry));
        }
        inner.retired.retain(|entry| !older(entry));
        inner.survivors.retain(|known, _| {
            !(known.sid == session.sid && known.started_at_secs < session.started_at_secs)
        });
        let list = inner.sessions.entry(agent_id.to_string()).or_default();
        if !list.iter().any(|(known, _)| *known == session) {
            list.push((session, folder));
        }
        if list.len() > SESSIONS_PER_AGENT {
            let excess = list.len() - SESSIONS_PER_AGENT;
            list.drain(..excess);
        }
    }

    /// [`Self::register`] for a standalone agent's session (see
    /// `standalone_owners`).
    pub fn register_standalone(
        &self,
        agent_id: &str,
        session: ProcessSession,
        folder: &std::path::Path,
    ) {
        self.lock().standalone_owners.insert(agent_id.to_string());
        self.register(agent_id, session, folder);
    }

    /// Mark `agent_id` as a standalone agent, so its sessions keep that kind.
    pub fn mark_standalone(&self, agent_id: &str) {
        self.lock().standalone_owners.insert(agent_id.to_string());
    }

    fn is_standalone_entry(
        inner: &RegistryInner,
        owner: Option<&str>,
        session: &ProcessSession,
    ) -> bool {
        owner.is_some_and(|owner| inner.standalone_owners.contains(owner))
            || inner.retired_standalone.contains(session)
    }

    /// Write survivors recorded from now on into the pending removals of
    /// `db_path`.
    pub fn set_pending_removal_store(&self, db_path: &std::path::Path) {
        self.lock().pending_store = Some(db_path.to_path_buf());
    }

    /// Record `identities` as members of `session` (see
    /// [`survivors_at_leader_exit`]), and into any pending removal of the
    /// folder the session runs in.
    pub fn record_survivors(&self, session: ProcessSession, identities: &[ProcessIdentity]) {
        if identities.is_empty() {
            return;
        }
        let (store, folder) = {
            let mut inner = self.lock();
            let entry = inner.survivors.entry(session).or_default();
            for identity in identities {
                if !entry.contains(identity) {
                    entry.push(*identity);
                }
            }
            let folder = inner
                .sessions
                .values()
                .flatten()
                .chain(inner.retired.iter())
                .find(|(known, _)| *known == session)
                .map(|(_, folder)| folder.clone());
            (inner.pending_store.clone(), folder)
        };
        if let (Some(store), Some(folder)) = (store, folder) {
            let written = crate::storage::SessionStore::open(&store)
                .and_then(|store| store.add_pending_removal_evidence(&folder, session, identities));
            if let Err(err) = written {
                crate::logger::warn(&format!(
                    "could not record what session {} left running into a pending removal: {err:#}",
                    session.sid
                ));
            }
        }
    }

    /// The recorded members of each of `sessions`.
    pub fn survivors_of(&self, sessions: &[ProcessSession]) -> Vec<ProcessIdentity> {
        let inner = self.lock();
        sessions
            .iter()
            .filter_map(|session| inner.survivors.get(session))
            .flatten()
            .copied()
            .collect()
    }

    /// A callback for a PTY's client to run once its child is gone: it records
    /// what is still running in the session, on a thread of its own because
    /// that is a walk over the process table.
    pub fn leader_exit_hook(&self, session: ProcessSession) -> Box<dyn FnOnce() + Send> {
        let registry = self.clone();
        Box::new(move || {
            let spawned = std::thread::Builder::new()
                .name("pty-leader-exit".to_string())
                .spawn(move || {
                    let found = survivors_at_leader_exit(session);
                    registry.record_survivors(session, &found);
                });
            if let Err(err) = spawned {
                crate::logger::debug(&format!(
                    "could not record what session {} left running: {err}",
                    session.sid
                ));
            }
        })
    }

    /// Every session remembered for `agent_id`.
    pub fn sessions_of(&self, agent_id: &str) -> Vec<ProcessSession> {
        self.lock()
            .sessions
            .get(agent_id)
            .map(|list| list.iter().map(|(session, _)| *session).collect())
            .unwrap_or_default()
    }

    /// Every session started in `folder` or anywhere inside it, whoever started
    /// it (compared on [`crate::worktree_ops::path_key`] keys by components): a live agent, a
    /// sibling agent sharing the folder, an agent already deleted whose
    /// processes may still be stopping, a terminal, a startup command. A
    /// removal of the folder ends all of them, and its last look before git
    /// runs asks this again to see whether anything new arrived.
    pub fn sessions_in(&self, folder: &std::path::Path) -> Vec<ProcessSession> {
        self.sessions_in_of_kind(folder, false)
    }

    /// The folders `sessions` were started in.
    pub fn folders_of(&self, sessions: &[ProcessSession]) -> Vec<std::path::PathBuf> {
        let inner = self.lock();
        let mut folders: Vec<std::path::PathBuf> = inner
            .sessions
            .values()
            .flatten()
            .chain(inner.retired.iter())
            .filter(|(session, _)| sessions.contains(session))
            .map(|(_, folder)| folder.clone())
            .collect();
        folders.sort();
        folders.dedup();
        folders
    }

    /// The sessions standalone agents started in `folder` or inside it, live or
    /// retired. Never ended by a removal; while they run they occupy the folder.
    pub fn standalone_sessions_in(&self, folder: &std::path::Path) -> Vec<ProcessSession> {
        self.sessions_in_of_kind(folder, true)
    }

    fn sessions_in_of_kind(
        &self,
        folder: &std::path::Path,
        standalone: bool,
    ) -> Vec<ProcessSession> {
        let folder = crate::worktree_ops::path_key(folder);
        let inner = self.lock();
        let live = inner
            .sessions
            .iter()
            .flat_map(|(owner, list)| list.iter().map(move |entry| (Some(owner.as_str()), entry)));
        let retired = inner.retired.iter().map(|entry| (None, entry));
        let mut found: Vec<ProcessSession> = live
            .chain(retired)
            .filter(|(_, (_, started_in))| started_in.starts_with(&folder))
            .filter(|(owner, (session, _))| {
                Self::is_standalone_entry(&inner, *owner, session) == standalone
            })
            .map(|(_, (session, _))| *session)
            .collect();
        found.sort_by_key(|session| session.sid);
        found.dedup();
        found
    }

    /// Forget an agent that has been deleted, handing back what it had, and
    /// flag a startup command still running for it, so that run neither
    /// recreates the agent's log folder nor reports a failure about an agent
    /// that is gone.
    ///
    /// A session whose id was handed out again LATER, to a session registered
    /// under any other key, is left out: once the agent's own session emptied,
    /// the kernel was free to reuse the number, and the newer session is
    /// somebody else's.
    pub fn forget_agent(&self, agent_id: &str) -> Vec<ProcessSession> {
        let mut inner = self.lock();
        if let Some(deleted) = inner.startup_runs.get_mut(agent_id) {
            *deleted = true;
        }
        let mine = inner.sessions.remove(agent_id).unwrap_or_default();
        let mine: Vec<(ProcessSession, std::path::PathBuf)> = mine
            .into_iter()
            .filter(|(session, _)| {
                !inner.sessions.values().flatten().any(|(other, _)| {
                    other.sid == session.sid && other.started_at_secs > session.started_at_secs
                })
            })
            .collect();
        let standalone = inner.standalone_owners.remove(agent_id);
        for entry in &mine {
            inner.retired.push_back(entry.clone());
            if standalone {
                inner.retired_standalone.insert(entry.0);
            }
        }
        while inner.retired.len() > RETIRED_SESSIONS {
            if let Some((dropped, _)) = inner.retired.pop_front() {
                inner.retired_standalone.remove(&dropped);
            }
        }
        mine.into_iter().map(|(session, _)| session).collect()
    }

    /// Claim the one startup-command run an agent may have at a time. `None`
    /// while another run is in progress; the guard releases the claim on drop.
    pub fn begin_startup_run(&self, agent_id: &str) -> Option<StartupRunGuard> {
        let mut inner = self.lock();
        if inner.startup_runs.contains_key(agent_id) {
            return None;
        }
        inner.startup_runs.insert(agent_id.to_string(), false);
        Some(StartupRunGuard {
            registry: self.clone(),
            agent_id: agent_id.to_string(),
        })
    }

    /// Whether an agent's startup command is running right now.
    pub fn startup_running(&self, agent_id: &str) -> bool {
        self.lock().startup_runs.contains_key(agent_id)
    }
}

/// The claim on an agent's one startup-command run. Registers the run's
/// session for the agent, says whether the agent was deleted while it ran, and
/// releases the claim when dropped.
pub struct StartupRunGuard {
    registry: AgentProcessRegistry,
    agent_id: String,
}

impl StartupRunGuard {
    pub fn register_session(&self, session: ProcessSession, folder: &std::path::Path) {
        self.registry.register(&self.agent_id, session, folder);
    }

    /// Record what the run's session still has running now that its leader,
    /// the command, has exited. Blocking: the run's own worker thread.
    pub fn record_survivors(&self, session: ProcessSession) {
        let found = survivors_at_leader_exit(session);
        self.registry.record_survivors(session, &found);
    }

    /// Whether the agent was deleted while this run was in progress.
    pub fn agent_deleted(&self) -> bool {
        self.registry
            .lock()
            .startup_runs
            .get(&self.agent_id)
            .copied()
            .unwrap_or(false)
    }
}

impl Drop for StartupRunGuard {
    fn drop(&mut self) {
        let mut inner = self.registry.lock();
        let deleted = inner.startup_runs.remove(&self.agent_id).unwrap_or(false);
        if deleted {
            // The agent's own forget already ran; a session registered after
            // it moves to the retired list, still findable by its folder.
            let late = inner.sessions.remove(&self.agent_id).unwrap_or_default();
            inner.retired.extend(late);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, ppid: u32, sid: u32) -> ProcRow {
        ProcRow {
            pid,
            ppid: Some(ppid),
            sid: Some(sid),
            start_time: 1_000,
            name: format!("p{pid}"),
            exited: false,
        }
    }

    fn session(sid: u32) -> ProcessSession {
        ProcessSession {
            sid,
            started_at_secs: 1_000,
            boot: current_boot(),
        }
    }

    fn pids(rows: &[ProcRow]) -> Vec<u32> {
        rows.iter().map(|r| r.pid).collect()
    }

    #[test]
    fn members_are_the_session_and_everything_below_it() {
        let table = vec![
            row(100, 1, 100), // the leader
            row(101, 100, 100),
            row(102, 101, 102), // called setsid, still a child
            row(103, 102, 102),
            row(200, 1, 200), // somebody else
        ];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[], 9)),
            vec![100, 101, 102, 103]
        );
    }

    /// The shell is gone and its background job was reparented to init. The
    /// job is still dux's through what was recorded when the shell exited,
    /// and never through the session number alone.
    #[test]
    fn a_session_outlives_its_leader_through_what_was_recorded_at_its_exit() {
        let job = row(101, 1, 100);
        let table = vec![job.clone(), row(200, 1, 200)];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[job.identity()], 9)),
            vec![101]
        );
        assert!(
            members(&table, &[session(100)], &[], 9).is_empty(),
            "never on the number alone"
        );
    }

    #[test]
    fn a_reused_session_id_names_somebody_else() {
        let mut leader = row(100, 1, 100);
        leader.start_time = 5_000;
        let table = vec![leader, row(101, 100, 100)];
        assert!(members(&table, &[session(100)], &[], 9).is_empty());
    }

    #[test]
    fn a_known_process_is_found_after_its_parent_link_is_cut() {
        let table = vec![row(102, 1, 102), row(103, 102, 102)];
        let known = [ProcessIdentity {
            pid: 102,
            start_time: 1_000,
        }];
        assert_eq!(
            pids(&members(&table, &[session(100)], &known, 9)),
            vec![102, 103]
        );
        // The same pid with another start time is a different process.
        let stale = [ProcessIdentity {
            pid: 102,
            start_time: 999,
        }];
        assert!(members(&table, &[session(100)], &stale, 9).is_empty());
    }

    #[test]
    fn a_session_from_another_boot_is_void() {
        let table = vec![row(100, 1, 100), row(101, 100, 100)];
        let earlier_boot = ProcessSession {
            boot: current_boot().wrapping_add(1),
            ..session(100)
        };
        assert!(members(&table, &[earlier_boot], &[], 9).is_empty());
    }

    #[test]
    fn nothing_that_started_before_the_session_is_in_it() {
        let mut before = row(101, 1, 100);
        before.start_time = 900;
        let table = vec![row(100, 1, 100), before, row(102, 1, 100)];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[], 9)),
            vec![100, 102]
        );
    }

    /// A session whose leader is gone is never trusted on its number, in this
    /// run or at the next start: the number may belong to a later, unrelated
    /// program by now. Only the processes recorded by identity, and what they
    /// started after them, are members.
    #[test]
    fn a_leaderless_session_needs_recorded_identities() {
        let mut reused = row(500, 1, 100);
        reused.start_time = 5_000;
        let mine = row(101, 1, 100);
        let child_of_mine = row(102, 101, 100);
        let mut older_than_its_parent = row(103, 101, 100);
        older_than_its_parent.start_time = 500;
        let table = vec![reused, mine.clone(), child_of_mine, older_than_its_parent];
        let recorded = [mine.identity()];
        assert_eq!(
            pids(&members(&table, &[session(100)], &recorded, 9)),
            vec![101, 102],
            "a child that started before its parent is a reused pid, not ours"
        );
        assert!(
            members(&table, &[session(100)], &[], 9).is_empty(),
            "never on the number alone"
        );
        // With its own leader still there, the session counts by number.
        let table = vec![row(100, 1, 100), row(101, 100, 100)];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[], 9)),
            vec![100, 101]
        );
    }

    #[test]
    fn a_newer_session_with_the_same_number_replaces_the_old_entry() {
        let registry = AgentProcessRegistry::default();
        let here = std::path::Path::new("/work/a1");
        let old = session(100);
        let newer = ProcessSession {
            started_at_secs: 2_000,
            ..session(100)
        };
        registry.register("a1", old, here);
        registry.register("a2", newer, std::path::Path::new("/work/a2"));
        assert!(registry.sessions_of("a1").is_empty());
        assert_eq!(registry.sessions_of("a2"), vec![newer]);
    }

    #[test]
    fn zombies_and_dux_itself_are_left_out() {
        let mut zombie = row(101, 100, 100);
        zombie.exited = true;
        let table = vec![
            row(100, 1, 100),
            zombie,
            row(9, 100, 100),
            row(104, 101, 100),
        ];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[], 9)),
            vec![100, 104]
        );
    }

    /// A scripted table: each process has a set of signals it dies to and a
    /// clock that only moves when the purge sleeps.
    struct Scripted {
        rows: Vec<(ProcRow, Vec<rustix::process::Signal>)>,
        sent: Vec<(u32, rustix::process::Signal)>,
        clock: Instant,
    }

    impl ProcessOps for Scripted {
        fn table(&mut self) -> Vec<ProcRow> {
            self.rows.iter().map(|(r, _)| r.clone()).collect()
        }
        fn signal(&mut self, pid: u32, signal: rustix::process::Signal) {
            self.sent.push((pid, signal));
            self.rows
                .retain(|(row, dies_to)| row.pid != pid || !dies_to.contains(&signal));
        }
        fn now(&self) -> Instant {
            self.clock
        }
        fn sleep(&mut self, duration: Duration) {
            self.clock += duration;
        }
    }

    #[test]
    fn purge_asks_first_and_kills_after_the_grace() {
        use rustix::process::Signal;
        let mut ops = Scripted {
            rows: vec![
                (row(100, 1, 100), vec![Signal::TERM]),
                (row(101, 1, 100), vec![Signal::KILL]),
            ],
            sent: Vec::new(),
            clock: Instant::now(),
        };
        // 101 is recorded by identity, so it stays a member after its leader
        // goes on SIGTERM.
        let known = [row(101, 1, 100).identity()];
        let outcome = purge(&mut ops, &[session(100)], &known, Duration::from_secs(1));
        assert_eq!(outcome, PurgeOutcome::Clean { stopped: 2 });
        assert!(ops.sent.contains(&(100, Signal::TERM)));
        assert!(ops.sent.contains(&(101, Signal::HUP)));
        assert!(
            !ops.sent.contains(&(100, Signal::KILL)),
            "a process that went on SIGTERM is never killed"
        );
        assert!(ops.sent.contains(&(101, Signal::KILL)));
    }

    #[test]
    fn purge_gives_up_on_a_process_that_will_not_die_and_names_it() {
        let start = Instant::now();
        let mut ops = Scripted {
            rows: vec![(row(101, 1, 100), Vec::new())],
            sent: Vec::new(),
            clock: start,
        };
        let known = [row(101, 1, 100).identity()];
        let outcome = purge(&mut ops, &[session(100)], &known, Duration::from_secs(1));
        match outcome {
            PurgeOutcome::Survivors(rows) => assert_eq!(pids(&rows), vec![101]),
            other => panic!("expected survivors, got {other:?}"),
        }
        let waited = ops.clock - start;
        assert!(
            waited >= Duration::from_secs(1) + KILL_SETTLE
                && waited < Duration::from_secs(1) + KILL_SETTLE + Duration::from_secs(1),
            "bounded by the grace plus the settle: {waited:?}"
        );
    }

    #[test]
    fn purge_of_nothing_is_immediate() {
        let mut ops = Scripted {
            rows: vec![(row(200, 1, 200), Vec::new())],
            sent: Vec::new(),
            clock: Instant::now(),
        };
        assert_eq!(
            purge(&mut ops, &[session(100)], &[], Duration::from_secs(5)),
            PurgeOutcome::Clean { stopped: 0 }
        );
        assert!(ops.sent.is_empty());
    }

    #[test]
    fn the_real_table_sees_a_real_session() {
        // A process of our own in a fresh session, found by session id alone.
        let mut child = std::process::Command::new("sh");
        child.args(["-c", "sleep 30"]);
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
        unsafe {
            use std::os::unix::process::CommandExt;
            child.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut child = child.spawn().expect("spawn sh");
        let sid = child.id();
        let found = members(
            &read_process_table(),
            &[ProcessSession::started_now(sid)],
            &[],
            0,
        );
        assert!(found.iter().any(|row| row.pid == sid), "{found:?}");
        let outcome = purge(
            &mut SystemProcesses,
            &[ProcessSession::started_now(sid)],
            &[],
            Duration::from_secs(1),
        );
        let _ = child.wait();
        assert!(matches!(outcome, PurgeOutcome::Clean { .. }), "{outcome:?}");
    }

    /// A session number the kernel handed out again, later, to somebody
    /// else's PTY is not the deleted agent's to end.
    #[test]
    fn a_session_id_reused_elsewhere_is_not_handed_back() {
        let registry = AgentProcessRegistry::default();
        let old = ProcessSession {
            sid: 100,
            started_at_secs: 1_000,
            boot: current_boot(),
        };
        let newer = ProcessSession {
            sid: 100,
            started_at_secs: 2_000,
            boot: current_boot(),
        };
        let here = std::path::Path::new("/work/a1");
        registry.register("a1", old, here);
        registry.register("a1", session(101), here);
        registry.register(UNOWNED_PTYS, newer, here);
        assert_eq!(registry.forget_agent("a1"), vec![session(101)]);
    }

    #[test]
    fn one_startup_run_per_agent_and_a_delete_is_seen() {
        let registry = AgentProcessRegistry::default();
        let guard = registry.begin_startup_run("a1").expect("first run");
        assert!(
            registry.begin_startup_run("a1").is_none(),
            "a second run is refused"
        );
        assert!(registry.begin_startup_run("a2").is_some(), "per agent");
        guard.register_session(session(100), std::path::Path::new("/work/a1"));
        assert!(!guard.agent_deleted());
        assert_eq!(registry.forget_agent("a1"), vec![session(100)]);
        assert!(guard.agent_deleted());
        drop(guard);
        assert!(!registry.startup_running("a1"));
        assert!(registry.sessions_of("a1").is_empty());
        assert_eq!(
            registry.sessions_in(std::path::Path::new("/work/a1")),
            vec![session(100)],
            "a deleted agent's session is still found by its folder"
        );
    }

    #[test]
    fn sessions_are_found_by_folder_whoever_started_them() {
        let registry = AgentProcessRegistry::default();
        let shared = std::path::Path::new("/work/shared");
        registry.register("a1", session(100), shared);
        registry.register("a2", session(200), shared);
        registry.register("a2", session(300), std::path::Path::new("/work/other"));
        assert_eq!(
            registry.sessions_in(shared),
            vec![session(100), session(200)]
        );
    }
}
