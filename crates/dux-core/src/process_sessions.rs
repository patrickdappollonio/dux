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
use std::time::{Duration, Instant};

/// How long a purge waits for a SIGKILL to land once it has been sent, on top
/// of the configured grace. A SIGKILLed process is gone as soon as the kernel
/// schedules it, so this only bounds the pathological case (a process stuck in
/// uninterruptible I/O) rather than an ordinary shutdown. Not a user setting:
/// the user's knob is the grace, and this is the floor under the step after it.
pub const KILL_SETTLE: Duration = Duration::from_secs(3);

/// How often a purge looks at the process table while it waits.
const PURGE_POLL: Duration = Duration::from_millis(50);

/// Slack allowed between the start time dux recorded for a session's leader
/// and the kernel's own, in nanoseconds since boot. dux reads the kernel's
/// own number whenever the leader can still be asked, so the two normally
/// agree exactly; the slack only covers the fallback, the moment of the spawn
/// on the same clock, when the leader was already gone.
const START_TIME_SLACK: u64 = 2_000_000_000;

/// The sessions whose leader dux spawned and has not reaped yet. dux is the
/// parent of every PTY's child and of every startup command, so until dux
/// itself reaps the leader its pid stays allocated (a zombie at worst) and the
/// session number cannot have been handed to anybody else: such a session is
/// led, whatever the process table says. That matters where the table cannot
/// show a zombie at all (macOS, where `sysinfo` leaves zombies out). The
/// table's own view of a live or zombie leader is the second source.
static UNREAPED: std::sync::LazyLock<Mutex<HashSet<ProcessSession>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// dux spawned `session`'s leader as its own child.
pub fn note_spawned(session: ProcessSession) {
    UNREAPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session);
}

/// dux reaped `session`'s leader: from now on its number proves nothing.
pub fn note_reaped(session: ProcessSession) {
    UNREAPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&session);
}

fn leader_unreaped(session: &ProcessSession) -> bool {
    UNREAPED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(session)
}

/// What [`judge_cwds`] found about one session's processes and a folder.
#[derive(Debug, PartialEq, Eq)]
enum CwdVerdict<'a> {
    /// Nothing of it stands in the folder.
    Clear,
    /// This process stands in the folder (or, its own folder unreadable, the
    /// nearest readable ancestor of it in its session does).
    Inside(&'a ProcRow),
    /// Where this process stands is unknown, nothing above it in its session
    /// can say, and the session was started in the folder: possibly in it.
    Unknown(&'a ProcRow),
}

/// Whether any of `running` (one session's live processes) stands in
/// `folder` (a path key). A process whose current folder cannot be read (a
/// `sudo` child, another user's process) is judged by its nearest readable
/// ancestor in the same session: the shell that ran it. Only when no such
/// ancestor is readable does it fail closed, and then only when the session
/// itself was started in or under the folder, so one unreadable process
/// somewhere else never blocks a removal here. Pure: the reads are handed in.
fn judge_cwds<'a>(
    folder: &std::path::Path,
    running: &'a [ProcRow],
    table: &[ProcRow],
    report: &crate::file_drop::CwdReport,
    session_folder: &std::path::Path,
) -> CwdVerdict<'a> {
    let by_pid: HashMap<u32, &ProcRow> = table.iter().map(|row| (row.pid, row)).collect();
    let inside = |cwd: &std::path::Path| crate::worktree_ops::path_key(cwd).starts_with(folder);
    let mut unknown: Option<&'a ProcRow> = None;
    for row in running {
        if let Some(cwd) = report.found.get(&row.pid) {
            if inside(cwd) {
                return CwdVerdict::Inside(row);
            }
            continue;
        }
        if !report.unknown.contains(&row.pid) {
            continue;
        }
        // Up the parent chain, staying in the session, to the first process
        // whose folder could be read.
        let mut seen = HashSet::new();
        let mut parent = row.ppid.and_then(|ppid| by_pid.get(&ppid).copied());
        let mut stand_in: Option<&std::path::PathBuf> = None;
        while let Some(ancestor) = parent {
            if ancestor.sid != row.sid || !seen.insert(ancestor.pid) {
                break;
            }
            if let Some(cwd) = report.found.get(&ancestor.pid) {
                stand_in = Some(cwd);
                break;
            }
            parent = ancestor.ppid.and_then(|ppid| by_pid.get(&ppid).copied());
        }
        match stand_in {
            Some(cwd) if inside(cwd) => return CwdVerdict::Inside(row),
            Some(_) => {}
            None => {
                if crate::worktree_ops::path_key(session_folder).starts_with(folder)
                    && unknown.is_none()
                {
                    unknown = Some(row);
                }
            }
        }
    }
    unknown.map_or(CwdVerdict::Clear, CwdVerdict::Unknown)
}

/// A start time the platform would not give (another user's process on
/// macOS). Every membership rule fails closed on it: such a process is a
/// member when its session or its parent chain says so, because a start time
/// that cannot be read cannot rule it out.
pub const UNKNOWN_START: u64 = 0;

/// A session dux created: a PTY's child or a startup command, both started
/// with `setsid`, so the session id is the leader's pid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ProcessSession {
    pub sid: u32,
    /// When the leader started, in nanoseconds since the machine booted (see
    /// [`process_start`]): a clock no wall-clock step can move, so a session
    /// is told apart from a later one that reused the same pid the same way
    /// whatever the clock on the wall did. Nothing that started before it can
    /// belong to it.
    pub started_at: u64,
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
            started_at: process_start(sid).unwrap_or_else(boot_nanos_now),
            boot: current_boot(),
        }
    }

    /// Whether this session was recorded in the boot the machine is in now.
    pub fn is_this_boot(&self) -> bool {
        self.boot == current_boot()
    }
}

/// An identity for the running boot of this machine: a hash of
/// `/proc/sys/kernel/random/boot_id` on Linux and of the `kern.bootsessionuuid`
/// sysctl on macOS, both of which change at every boot and at nothing else.
/// (`kern.boottime` is not used: it is derived from the wall clock and moves
/// when the clock is stepped, without a reboot.) Only when neither can be
/// read does it fall back to the boot time. Read once per process.
pub fn current_boot() -> u64 {
    static BOOT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        let id = platform_boot_id()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| sysinfo::System::boot_time().to_string());
        // FNV-1a: stable across runs and builds, unlike the std hasher.
        id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

#[cfg(target_os = "linux")]
fn platform_boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .ok()
}

#[cfg(target_os = "macos")]
fn platform_boot_id() -> Option<String> {
    let mut buf = [0u8; 128];
    let mut len: libc::size_t = buf.len();
    // SAFETY: the name is NUL-terminated, the buffer and its length describe
    // writable memory, and no new value is set.
    let rc = unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let text = buf.get(..len.min(buf.len()))?;
    let text = text.split(|byte| *byte == 0).next()?;
    std::str::from_utf8(text)
        .ok()
        .map(|text| text.trim().to_string())
}

/// Now, in nanoseconds since the machine booted, on the clock process start
/// times are read in: `CLOCK_BOOTTIME` on Linux (what `/proc/<pid>/stat`'s
/// start time counts), the Mach absolute clock on macOS (what the kernel
/// stamps a process's start with). Neither is stepped when the wall clock is.
pub fn boot_nanos_now() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let now = rustix::time::clock_gettime(rustix::time::ClockId::Boottime);
        (now.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(now.tv_nsec as u64)
    }
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a plain read of the Mach clock, no arguments.
        #[allow(deprecated)]
        let now = unsafe { libc::mach_absolute_time() };
        mach_to_nanos(now)
    }
}

/// When process `pid` started, in nanoseconds since boot (see
/// [`boot_nanos_now`]), or `None` when it cannot be asked (gone, or not
/// ours to ask on macOS).
pub fn process_start(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_linux_stat(pid, &stat).map(|row| row.start_time)
    }
    #[cfg(target_os = "macos")]
    {
        macos_start(pid)
    }
}

/// Clock ticks per second, the unit of `/proc/<pid>/stat`'s start time.
#[cfg(target_os = "linux")]
fn clock_ticks_per_second() -> u64 {
    static TICKS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TICKS.get_or_init(|| {
        // SAFETY: sysconf reads a constant of the running system.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        u64::try_from(ticks).ok().filter(|t| *t > 0).unwrap_or(100)
    })
}

/// One `/proc/<pid>/stat` line as a row. The command name sits in
/// parentheses and may itself contain spaces and parentheses, so the fields
/// after it are read from after the LAST closing parenthesis.
#[cfg(any(target_os = "linux", test))]
fn parse_linux_stat_with(pid: u32, stat: &str, ticks_per_second: u64) -> Option<ProcRow> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    // Fields after the name, numbered from 3 in proc(5): state (3), ppid (4),
    // pgrp (5), session (6), ... starttime (22).
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    let state = fields.first()?;
    let ppid: u32 = fields.get(1)?.parse().ok()?;
    let sid: u32 = fields.get(3)?.parse().ok()?;
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    Some(ProcRow {
        pid,
        ppid: (ppid != 0).then_some(ppid),
        sid: Some(sid),
        start_time: (u128::from(ticks) * 1_000_000_000 / u128::from(ticks_per_second.max(1)))
            as u64,
        name,
        exited: matches!(*state, "Z" | "X" | "x"),
    })
}

#[cfg(target_os = "linux")]
fn parse_linux_stat(pid: u32, stat: &str) -> Option<ProcRow> {
    parse_linux_stat_with(pid, stat, clock_ticks_per_second())
}

/// Mach absolute time in nanoseconds.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn mach_to_nanos(abstime: u64) -> u64 {
    static TIMEBASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    let (numer, denom) = *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: fills the struct it is handed.
        let rc = unsafe { libc::mach_timebase_info(&mut info) };
        if rc == 0 && info.denom != 0 {
            (info.numer, info.denom)
        } else {
            (1, 1)
        }
    });
    (u128::from(abstime) * u128::from(numer) / u128::from(denom)) as u64
}

/// A process's start on the Mach absolute clock, in nanoseconds.
#[cfg(target_os = "macos")]
fn macos_start(pid: u32) -> Option<u64> {
    let pid = libc::c_int::try_from(pid).ok()?;
    // SAFETY: zeroed is a valid value for this plain-data struct, and the
    // kernel writes at most its size for the V2 flavor.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast::<libc::rusage_info_t>(),
        )
    };
    (rc == 0).then(|| mach_to_nanos(info.ri_proc_start_abstime))
}

/// One process, identified across two looks at the table: a pid alone can be
/// reused, a pid with its start time cannot (in practice).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
    /// The machine boot it was recorded in ([`current_boot`]). An identity
    /// from another boot is void, exactly like a session: every pid and
    /// every boot-relative start time has been handed out afresh since, so
    /// it names nothing of dux's however well it matches. An identity read
    /// back without one (recorded before this field existed) is void too.
    #[serde(default)]
    pub boot: u64,
}

/// One row of the process table, as much of it as these questions need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// `None` when the process exited between the listing and the question.
    pub sid: Option<u32>,
    /// When it started, in nanoseconds since boot (see [`process_start`]); 0
    /// when the platform would not say (another user's process on macOS).
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
            boot: current_boot(),
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
            // A zombie leader still counts: its pid stays allocated until it
            // is reaped, so its session number cannot have been handed out
            // again. A start time the platform would not give (another
            // user's process on macOS) fails closed: it counts.
            leader_unreaped(session)
                || by_pid.get(&session.sid).is_some_and(|leader| {
                    leader.sid == Some(leader.pid)
                        && (leader.start_time == UNKNOWN_START
                            || (leader.start_time + START_TIME_SLACK >= session.started_at
                                && leader.start_time <= session.started_at + START_TIME_SLACK))
                })
        })
        .map(|session| {
            (
                session.sid,
                session.started_at.saturating_sub(START_TIME_SLACK),
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
                .is_some_and(|earliest| {
                    row.start_time == UNKNOWN_START || row.start_time >= *earliest
                })
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
                kids.iter().copied().filter(|kid| {
                    kid.start_time == UNKNOWN_START || kid.start_time >= row.start_time
                }),
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
    let earliest = session.started_at.saturating_sub(START_TIME_SLACK);
    read_process_table()
        .iter()
        .filter(|row| {
            !row.exited
                && row.sid == Some(session.sid)
                && (row.start_time == UNKNOWN_START || row.start_time >= earliest)
                && row.pid != std::process::id()
        })
        .map(ProcRow::identity)
        .collect()
}

/// The process table, read off the platform: `/proc` itself on Linux (the
/// only place the start time is given in boot-relative ticks), and libproc on
/// macOS (through `sysinfo`, with each start read off the Mach clock).
/// Blocking file and syscall work: call it from a worker thread, never the UI
/// thread.
pub fn read_process_table() -> Vec<ProcRow> {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                parse_linux_stat(pid, &stat)
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        sys.processes()
            .iter()
            .filter(|(_, process)| process.thread_kind().is_none())
            .map(|(pid, process)| ProcRow {
                pid: pid.as_u32(),
                ppid: process.parent().map(|parent| parent.as_u32()),
                sid: process.session_id().map(|sid| sid.as_u32()),
                start_time: macos_start(pid.as_u32()).unwrap_or(UNKNOWN_START),
                name: process.name().to_string_lossy().into_owned(),
                exited: matches!(
                    process.status(),
                    ProcessStatus::Zombie | ProcessStatus::Dead
                ),
            })
            .collect()
    }
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

/// One write for the registry's writer thread, numbered in the order the
/// registry changed.
enum WriteJob {
    /// The whole registry, into its own table.
    Registry {
        seq: u64,
        stored: Vec<StoredSession>,
    },
    /// What the registry knows in a pending removal's folder, merged into
    /// its row.
    Pending {
        seq: u64,
        row: String,
        snapshot: RegistrySnapshot,
    },
}

/// The first pause before a failed database write is tried again, and the
/// longest it grows to.
const WRITE_RETRY_FIRST: Duration = Duration::from_millis(100);
const WRITE_RETRY_MAX: Duration = Duration::from_secs(2);

/// How far the writer thread has got.
#[derive(Default)]
struct WriterProgress {
    /// The highest change number written (or given up on, with a warning).
    done: Mutex<u64>,
    advanced: std::sync::Condvar,
}

/// The registry's database writer: one thread with one long-lived
/// connection. Changes are handed to it and never waited for on the thread
/// that made them (the engine's), so a database another process or thread
/// has locked can slow the writes down but never the UI. Consecutive
/// whole-registry writes coalesce, the latest winning; pending-row merges are
/// written in order.
struct RegistryWriter {
    tx: std::sync::mpsc::Sender<WriteJob>,
    progress: Arc<WriterProgress>,
}

impl RegistryWriter {
    fn spawn(db_path: std::path::PathBuf) -> Option<Self> {
        let (tx, rx) = std::sync::mpsc::channel::<WriteJob>();
        let progress = Arc::new(WriterProgress::default());
        let thread_progress = Arc::clone(&progress);
        std::thread::Builder::new()
            .name("process-registry-writer".to_string())
            .spawn(move || Self::run(&db_path, &rx, &thread_progress))
            .map_err(|err| {
                crate::logger::warn(&format!(
                    "could not start saving the process sessions dux starts: {err}"
                ));
            })
            .ok()?;
        Some(Self { tx, progress })
    }

    fn run(
        db_path: &std::path::Path,
        rx: &std::sync::mpsc::Receiver<WriteJob>,
        progress: &WriterProgress,
    ) {
        let mut store: Option<crate::storage::SessionStore> = None;
        // What is waiting to be written: the latest whole registry, and every
        // pending-row merge in order. A write that fails stays here and is
        // tried again, with a growing pause, so nothing is counted as saved
        // until it is in the database.
        let mut registry: Option<(u64, Vec<StoredSession>)> = None;
        let mut merges: std::collections::VecDeque<(u64, String, RegistrySnapshot)> =
            std::collections::VecDeque::new();
        let mut highest = 0;
        let mut backoff = WRITE_RETRY_FIRST;
        let mut open = true;
        loop {
            let nothing_waiting = registry.is_none() && merges.is_empty();
            if nothing_waiting {
                if !open {
                    return;
                }
                match rx.recv() {
                    Ok(job) => Self::take(job, &mut registry, &mut merges, &mut highest),
                    // Every registry handle is gone and everything is written.
                    Err(_) => return,
                }
            } else if open {
                // Something failed: wait before trying again, taking whatever
                // arrives meanwhile.
                match rx.recv_timeout(backoff) {
                    Ok(job) => Self::take(job, &mut registry, &mut merges, &mut highest),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => open = false,
                }
            } else {
                std::thread::sleep(backoff);
            }
            for job in rx.try_iter() {
                Self::take(job, &mut registry, &mut merges, &mut highest);
            }
            if store.is_none() {
                match crate::storage::SessionStore::open(db_path) {
                    Ok(opened) => store = Some(opened),
                    Err(err) => crate::logger::warn(&format!(
                        "could not open the session database to save the process sessions dux \
                         started; trying again: {err:#}"
                    )),
                }
            }
            let mut failed = store.is_none();
            if let Some(store) = &store {
                while let Some((_, row, snapshot)) = merges.front() {
                    match store.merge_pending_removal_registry(row, snapshot) {
                        Ok(()) => {
                            merges.pop_front();
                        }
                        Err(err) => {
                            crate::logger::warn(&format!(
                                "could not keep a pending removal current with what runs in its \
                                 folder; trying again: {err:#}"
                            ));
                            failed = true;
                            break;
                        }
                    }
                }
                if !failed && let Some((_, stored)) = &registry {
                    match store.replace_process_registry(stored) {
                        Ok(()) => registry = None,
                        Err(err) => {
                            crate::logger::warn(&format!(
                                "could not save the process sessions dux started; trying \
                                 again: {err:#}"
                            ));
                            failed = true;
                        }
                    }
                }
            }
            // Everything numbered below the oldest write still waiting is in.
            let oldest_waiting = merges
                .front()
                .map(|(seq, ..)| *seq)
                .into_iter()
                .chain(registry.as_ref().map(|(seq, _)| *seq))
                .min();
            let through = oldest_waiting.map_or(highest, |seq| seq.saturating_sub(1));
            {
                let mut done = progress
                    .done
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *done = (*done).max(through);
                progress.advanced.notify_all();
            }
            backoff = if failed {
                (backoff * 2).min(WRITE_RETRY_MAX)
            } else {
                WRITE_RETRY_FIRST
            };
        }
    }

    /// File one job: a whole registry replaces the one waiting (the latest
    /// wins), a merge joins the queue in order.
    fn take(
        job: WriteJob,
        registry: &mut Option<(u64, Vec<StoredSession>)>,
        merges: &mut std::collections::VecDeque<(u64, String, RegistrySnapshot)>,
        highest: &mut u64,
    ) {
        match job {
            WriteJob::Registry { seq, stored } => {
                *highest = (*highest).max(seq);
                // The registry waiting keeps its OLDER number: the record is
                // not saved through it until a write of the newer one lands.
                let seq = registry.as_ref().map_or(seq, |(older, _)| *older);
                *registry = Some((seq, stored));
            }
            WriteJob::Pending { seq, row, snapshot } => {
                *highest = (*highest).max(seq);
                merges.push_back((seq, row, snapshot));
            }
        }
    }
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
    /// The session database: the whole registry is written into its own
    /// table there on every change and read back at the next start, and
    /// pending removals are kept current in it.
    pending_store: Option<std::path::PathBuf>,
    /// The writer thread, once a database is attached.
    writer: Option<RegistryWriter>,
    /// The number of the latest change handed to the writer.
    queued: u64,
    /// Leader-exit recordings started and not yet written, by session: a
    /// removal reading what was recorded for a session waits for these first.
    recording: HashMap<ProcessSession, usize>,
    /// The pending removals being kept current, by row id, with the folder
    /// each removes. Every change to the registry in or under one of these
    /// folders is written into its row at the moment it happens (see
    /// [`AgentProcessRegistry::sync_pending`]), so a later start ends exactly
    /// the set the live run would have.
    pending_folders: HashMap<String, std::path::PathBuf>,
    /// What to call each session's processes in a sentence ("project
    /// terminal Terminal 2", "agent X's startup command"), for the ones dux
    /// can name. Runtime only: a session read back from an earlier run is
    /// "a process dux started".
    labels: HashMap<ProcessSession, String>,
    /// A prune of ended sessions is running because a list went past its cap.
    cap_prune_running: bool,
    /// The registry went past a cap with every session still live, and that
    /// has been logged.
    cap_overrun_logged: bool,
    /// The session of each agent's startup command that is running right now.
    startup_sessions: HashMap<String, ProcessSession>,
    /// Told, in a sentence, when a deleted agent's startup command would not
    /// stop (the engine raises it as a warning), and the grace it is given.
    startup_stop: Option<(Duration, StartupStopNotice)>,
}

/// Who is told, in a sentence, that a deleted agent's startup command would
/// not stop.
type StartupStopNotice = Arc<dyn Fn(String) + Send + Sync>;

/// One registered session, as a pending removal's row records it: the
/// session, the folder it was started in, and its owner's kind (a standalone
/// agent's sessions are never ended, live or retired, in this run or the next).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegistryEntry {
    pub session: ProcessSession,
    pub folder: std::path::PathBuf,
    pub standalone: bool,
}

/// One registry entry as the registry's own table keeps it across restarts.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredSession {
    /// The agent (or `UNOWNED_PTYS`) it belongs to; `None` once retired.
    pub owner: Option<String>,
    pub session: ProcessSession,
    pub folder: std::path::PathBuf,
    pub standalone: bool,
    pub survivors: Vec<ProcessIdentity>,
    /// What its processes are called in a sentence, so a refusal after a
    /// restart still names what is standing in a folder.
    #[serde(default)]
    pub label: Option<String>,
}

/// The part of the registry a removal of one folder depends on, written into
/// its pending row and rebuilt into a registry of the same type at the next
/// start, so the resumed removal asks exactly the questions the live run would.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegistrySnapshot {
    pub entries: Vec<RegistryEntry>,
    pub survivors: Vec<(ProcessSession, Vec<ProcessIdentity>)>,
}

impl RegistrySnapshot {
    /// Everything in `self` and in `other`, each entry and identity once.
    pub fn merge(&mut self, other: &RegistrySnapshot) {
        for entry in &other.entries {
            if !self
                .entries
                .iter()
                .any(|known| known.session == entry.session)
            {
                self.entries.push(entry.clone());
            } else if entry.standalone {
                // Owner kind is for life: once standalone, always standalone.
                for known in &mut self.entries {
                    if known.session == entry.session {
                        known.standalone = true;
                    }
                }
            }
        }
        for (session, identities) in &other.survivors {
            match self
                .survivors
                .iter_mut()
                .find(|(known, _)| known == session)
            {
                Some((_, known)) => {
                    for identity in identities {
                        if !known.contains(identity) {
                            known.push(*identity);
                        }
                    }
                }
                None => self.survivors.push((*session, identities.clone())),
            }
        }
    }
}

/// The registry key for a PTY that belongs to no agent (a project or a
/// standalone terminal). It is never purged; it is registered so a deleted
/// agent's old session number, reused for one of these, is recognised as
/// somebody else's.
pub const UNOWNED_PTYS: &str = "\u{0}unowned";

/// How many sessions of deleted agents are remembered before the ones
/// confirmed ended are dropped. A soft cap: see [`SESSIONS_PER_AGENT`].
const RETIRED_SESSIONS: usize = 1024;

/// How many sessions are remembered per agent before the ones confirmed ended
/// are dropped. A soft cap: a session with anything still running in it is
/// never forgotten (a removal of its folder only ends what is listed), so a
/// list whose every session is still live grows past it, and that is logged
/// once.
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
            known.sid == session.sid && known.started_at < session.started_at
        };
        for list in inner.sessions.values_mut() {
            list.retain(|entry| !older(entry));
        }
        inner.retired.retain(|entry| !older(entry));
        inner.survivors.retain(|known, _| {
            !(known.sid == session.sid && known.started_at < session.started_at)
        });
        let list = inner.sessions.entry(agent_id.to_string()).or_default();
        if !list.iter().any(|(known, _)| *known == session) {
            list.push((session, folder.clone()));
        }
        drop(inner);
        self.sync_pending(&folder);
        self.enforce_caps();
    }

    /// Remember what to call `session`'s processes in a sentence.
    pub fn label(&self, session: ProcessSession, label: impl Into<String>) {
        self.lock().labels.insert(session, label.into());
        self.persist_all();
    }

    /// Whether a list is past its cap.
    fn over_cap(inner: &RegistryInner) -> bool {
        inner.retired.len() > RETIRED_SESSIONS
            || inner
                .sessions
                .values()
                .any(|list| list.len() > SESSIONS_PER_AGENT)
    }

    /// Past a cap, drop the sessions confirmed ended, off the calling thread
    /// (it is a look at the process table). Nothing with a live process is
    /// ever dropped; if the lists are still past their caps afterwards, that
    /// is logged once.
    fn enforce_caps(&self) {
        {
            let mut inner = self.lock();
            if !Self::over_cap(&inner) || inner.cap_prune_running {
                return;
            }
            inner.cap_prune_running = true;
        }
        let registry = self.clone();
        let spawned = std::thread::Builder::new()
            .name("process-registry-cap".to_string())
            .spawn(move || {
                registry.prune_gone();
                let mut inner = registry.lock();
                inner.cap_prune_running = false;
                if Self::over_cap(&inner) && !inner.cap_overrun_logged {
                    inner.cap_overrun_logged = true;
                    drop(inner);
                    crate::logger::info(
                        "dux is following more process sessions than its usual limit, because \
                         every one of them still has something running; none is forgotten \
                         while it does",
                    );
                }
            });
        if spawned.is_err() {
            self.lock().cap_prune_running = false;
        }
    }

    /// Forget `sessions` everywhere, now that nothing runs in them any more
    /// (a failed create's startup command, ended before its worktree was
    /// rolled back).
    pub fn unregister(&self, sessions: &[ProcessSession]) {
        let folders: Vec<std::path::PathBuf> = {
            let mut inner = self.lock();
            let mut folders = Vec::new();
            for list in inner.sessions.values_mut() {
                list.retain(|(session, folder)| {
                    let gone = sessions.contains(session);
                    if gone {
                        folders.push(folder.clone());
                    }
                    !gone
                });
            }
            inner.sessions.retain(|_, list| !list.is_empty());
            inner.retired.retain(|(session, folder)| {
                let gone = sessions.contains(session);
                if gone {
                    folders.push(folder.clone());
                }
                !gone
            });
            for session in sessions {
                inner.survivors.remove(session);
                inner.labels.remove(session);
                inner.retired_standalone.remove(session);
            }
            folders
        };
        for folder in folders {
            self.sync_pending(&folder);
        }
    }

    /// The first process dux started that stands in `folder` or under it as
    /// its working directory, among every session dux follows except
    /// `except` (the ones the caller is about to end itself): a terminal whose
    /// shell was `cd`'d there, a server a closed tab left running there. The
    /// answer is a phrase naming it, for a refusal. Blocking: reads the
    /// process table and each member's working directory.
    pub fn cwd_occupant(
        &self,
        folder: &std::path::Path,
        except: &[ProcessSession],
    ) -> Option<String> {
        type Tracked = (
            ProcessSession,
            std::path::PathBuf,
            Vec<ProcessIdentity>,
            Option<String>,
        );
        let tracked: Vec<Tracked> = {
            let inner = self.lock();
            let mut all: Vec<(ProcessSession, std::path::PathBuf)> = inner
                .sessions
                .values()
                .flatten()
                .chain(inner.retired.iter())
                .filter(|(session, _)| !except.contains(session))
                .cloned()
                .collect();
            all.sort_by_key(|(session, _)| (session.sid, session.started_at));
            all.dedup_by_key(|(session, _)| *session);
            all.into_iter()
                .map(|(session, started_in)| {
                    (
                        session,
                        started_in,
                        inner.survivors.get(&session).cloned().unwrap_or_default(),
                        inner.labels.get(&session).cloned(),
                    )
                })
                .collect()
        };
        if tracked.is_empty() {
            return None;
        }
        let folder = crate::worktree_ops::path_key(folder);
        let table = read_process_table();
        let self_pid = std::process::id();
        for (session, started_in, known, label) in tracked {
            let running = members(&table, &[session], &known, self_pid);
            if running.is_empty() {
                continue;
            }
            let pids: Vec<u32> = running.iter().map(|row| row.pid).collect();
            let report = crate::file_drop::process_cwds(&pids);
            let who = || {
                label
                    .clone()
                    .unwrap_or_else(|| "a process dux started".to_string())
            };
            match judge_cwds(&folder, &running, &table, &report, &started_in) {
                CwdVerdict::Clear => {}
                CwdVerdict::Inside(row) => {
                    return Some(format!(
                        "{} is working in it ({} (pid {}) has it as its current folder); `cd` \
                         out of it or close it first",
                        who(),
                        row.name,
                        row.pid
                    ));
                }
                CwdVerdict::Unknown(row) => {
                    return Some(format!(
                        "dux could not check where its terminals and processes are working: it \
                         could not read the current folder of {} (pid {}), started by {} in \
                         this folder{}, so it cannot rule out that it is still standing here; \
                         close it or try again",
                        row.name,
                        row.pid,
                        who(),
                        report
                            .failure
                            .as_deref()
                            .map(|why| format!(" ({why})"))
                            .unwrap_or_default()
                    ));
                }
            }
        }
        None
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
        let folders: Vec<std::path::PathBuf> = {
            let mut inner = self.lock();
            inner.standalone_owners.insert(agent_id.to_string());
            inner
                .sessions
                .get(agent_id)
                .map(|list| list.iter().map(|(_, folder)| folder.clone()).collect())
                .unwrap_or_default()
        };
        for folder in folders {
            self.sync_pending(&folder);
        }
    }

    fn is_standalone_entry(
        inner: &RegistryInner,
        owner: Option<&str>,
        session: &ProcessSession,
    ) -> bool {
        owner.is_some_and(|owner| inner.standalone_owners.contains(owner))
            || inner.retired_standalone.contains(session)
    }

    /// Keep the pending removal `row_id` of `folder` current in `db_path`
    /// from now on, and write what the registry already knows into it.
    pub fn watch_pending(&self, row_id: &str, folder: &std::path::Path, db_path: &std::path::Path) {
        {
            let mut inner = self.lock();
            Self::ensure_writer(&mut inner, db_path);
            inner
                .pending_folders
                .insert(row_id.to_string(), crate::worktree_ops::path_key(folder));
        }
        self.sync_pending(folder);
    }

    /// Stop keeping `row_id` current: its removal has run.
    pub fn unwatch_pending(&self, row_id: &str) {
        self.lock().pending_folders.remove(row_id);
    }

    /// Start the writer for `db_path`, if none runs yet.
    fn ensure_writer(inner: &mut RegistryInner, db_path: &std::path::Path) {
        if inner.writer.is_none() {
            inner.pending_store = Some(db_path.to_path_buf());
            inner.writer = RegistryWriter::spawn(db_path.to_path_buf());
        }
    }

    /// Hand the writer one job, numbered now, under the registry's lock, so
    /// the numbers follow the order the registry changed in.
    fn enqueue(inner: &mut RegistryInner, job: impl FnOnce(u64) -> WriteJob) {
        let Some(writer) = &inner.writer else {
            return;
        };
        let seq = inner.queued + 1;
        if writer.tx.send(job(seq)).is_ok() {
            inner.queued = seq;
        }
    }

    /// THE write path into the database: every registry change lands here,
    /// at the moment it happens, as a job for the writer thread. The whole
    /// registry goes into its own table, and what is known in or under a
    /// folder with a pending removal into that removal's row. Never waits.
    fn sync_pending(&self, changed: &std::path::Path) {
        let changed = crate::worktree_ops::path_key(changed);
        let mut inner = self.lock();
        if inner.writer.is_none() {
            return;
        }
        let stored = Self::stored_locked(&inner);
        Self::enqueue(&mut inner, |seq| WriteJob::Registry { seq, stored });
        let rows: Vec<(String, std::path::PathBuf)> = inner
            .pending_folders
            .iter()
            .filter(|(_, folder)| changed.starts_with(folder))
            .map(|(row, folder)| (row.clone(), folder.clone()))
            .collect();
        for (row, folder) in rows {
            let snapshot = Self::snapshot_in_locked(&inner, &folder);
            Self::enqueue(&mut inner, |seq| WriteJob::Pending { seq, row, snapshot });
        }
    }

    /// Hand the whole registry to the writer. Part of the one write path.
    fn persist_all(&self) {
        let mut inner = self.lock();
        if inner.writer.is_none() {
            return;
        }
        let stored = Self::stored_locked(&inner);
        Self::enqueue(&mut inner, |seq| WriteJob::Registry { seq, stored });
    }

    /// The registry as its table keeps it.
    fn stored_locked(inner: &RegistryInner) -> Vec<StoredSession> {
        let live = inner
            .sessions
            .iter()
            .flat_map(|(owner, list)| list.iter().map(move |entry| (Some(owner.clone()), entry)));
        let retired = inner.retired.iter().map(|entry| (None, entry));
        live.chain(retired)
            .map(|(owner, (session, folder))| StoredSession {
                standalone: Self::is_standalone_entry(inner, owner.as_deref(), session),
                owner,
                session: *session,
                folder: folder.clone(),
                survivors: inner.survivors.get(session).cloned().unwrap_or_default(),
                label: inner.labels.get(session).cloned(),
            })
            .collect()
    }

    /// Wait, at most `timeout`, until every change made so far is in the
    /// database. `true` once it is (at once when no database is attached).
    /// A removal asks this before git runs, so the record a later start
    /// recovers from is never behind a deletion that already happened.
    /// Blocking: a worker thread's call.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (target, progress) = {
            let inner = self.lock();
            match &inner.writer {
                Some(writer) => (inner.queued, Arc::clone(&writer.progress)),
                None => return true,
            }
        };
        let deadline = Instant::now() + timeout;
        let mut done = progress
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *done < target {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            done = progress
                .advanced
                .wait_timeout(done, deadline - now)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Attach the session database: read back what an earlier run of dux
    /// started in this boot (a session from another boot is void, every pid
    /// having been handed out afresh since), write every change from now on,
    /// and drop, off the calling thread, every session nothing of which is
    /// still running.
    pub fn attach_store(&self, db_path: &std::path::Path) {
        let loaded = crate::storage::SessionStore::open(db_path)
            .and_then(|store| store.load_process_registry());
        {
            let mut inner = self.lock();
            Self::ensure_writer(&mut inner, db_path);
            match loaded {
                Ok(stored) => {
                    for entry in stored.into_iter().filter(|e| e.session.is_this_boot()) {
                        let known = (entry.session, entry.folder.clone());
                        match &entry.owner {
                            Some(owner) => {
                                let list = inner.sessions.entry(owner.clone()).or_default();
                                if !list.contains(&known) {
                                    list.push(known);
                                }
                                if entry.standalone {
                                    inner.standalone_owners.insert(owner.clone());
                                }
                            }
                            None => {
                                inner.retired.push_back(known);
                                if entry.standalone {
                                    inner.retired_standalone.insert(entry.session);
                                }
                            }
                        }
                        if !entry.survivors.is_empty() {
                            inner.survivors.insert(entry.session, entry.survivors);
                        }
                        if let Some(label) = entry.label {
                            inner.labels.insert(entry.session, label);
                        }
                    }
                }
                Err(err) => crate::logger::warn(&format!(
                    "could not read back the process sessions an earlier run started: {err:#}"
                )),
            }
        }
        let registry = self.clone();
        let spawned = std::thread::Builder::new()
            .name("process-registry-prune".to_string())
            .spawn(move || registry.prune_gone());
        if let Err(err) = spawned {
            crate::logger::debug(&format!("could not prune the process registry: {err}"));
        }
    }

    /// Drop every session nothing of which is still running, under the one
    /// membership rule, and save the result. Blocking: a worker thread's call.
    pub fn prune_gone(&self) {
        self.prune_gone_with(read_process_table);
    }

    /// [`Self::prune_gone`] against the table `read` returns.
    fn prune_gone_with(&self, read: impl FnOnce() -> Vec<ProcRow>) {
        // Only what was registered BEFORE the table is read can be judged by
        // it: a session registered after the read is missing from that table
        // because it did not exist yet, not because it ended.
        let candidates: HashSet<ProcessSession> = {
            let inner = self.lock();
            inner
                .sessions
                .values()
                .flatten()
                .chain(inner.retired.iter())
                .map(|(session, _)| *session)
                .collect()
        };
        let table = read();
        let gone: HashSet<ProcessSession> = {
            let inner = self.lock();
            candidates
                .into_iter()
                .filter(|session| {
                    let known = inner.survivors.get(session).cloned().unwrap_or_default();
                    members(&table, &[*session], &known, std::process::id()).is_empty()
                        && !inner.recording.contains_key(session)
                })
                .collect()
        };
        if gone.is_empty() {
            self.persist_all();
            return;
        }
        {
            let mut inner = self.lock();
            for list in inner.sessions.values_mut() {
                list.retain(|(session, _)| !gone.contains(session));
            }
            inner.retired.retain(|(session, _)| !gone.contains(session));
            inner.survivors.retain(|session, _| !gone.contains(session));
            inner.labels.retain(|session, _| !gone.contains(session));
            inner
                .retired_standalone
                .retain(|session| !gone.contains(session));
        }
        self.persist_all();
    }

    /// Note that a leader-exit recording for `session` has started.
    fn begin_recording(&self, session: ProcessSession) {
        *self.lock().recording.entry(session).or_default() += 1;
    }

    fn end_recording(&self, session: ProcessSession) {
        let mut inner = self.lock();
        if let Some(count) = inner.recording.get_mut(&session) {
            *count -= 1;
            if *count == 0 {
                inner.recording.remove(&session);
            }
        }
    }

    /// Wait, bounded, until every leader-exit recording started for one of
    /// `sessions` has been written, so a removal never reads what was recorded
    /// for a session before the recording of its leader's exit has landed.
    pub fn wait_for_recordings(&self, sessions: &[ProcessSession], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let pending = {
                let inner = self.lock();
                sessions
                    .iter()
                    .any(|session| inner.recording.contains_key(session))
            };
            if !pending {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// [`Self::wait_for_recordings`] for every recording in flight.
    pub fn wait_for_all_recordings(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline && !self.lock().recording.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Everything the registry knows in `folder` or under it: every session
    /// (live or retired, any owner) with its owner's kind, and the members
    /// recorded for each.
    pub fn snapshot_in(&self, folder: &std::path::Path) -> RegistrySnapshot {
        let inner = self.lock();
        Self::snapshot_in_locked(&inner, folder)
    }

    fn snapshot_in_locked(inner: &RegistryInner, folder: &std::path::Path) -> RegistrySnapshot {
        let folder = crate::worktree_ops::path_key(folder);
        let live = inner
            .sessions
            .iter()
            .flat_map(|(owner, list)| list.iter().map(move |entry| (Some(owner.as_str()), entry)));
        let retired = inner.retired.iter().map(|entry| (None, entry));
        let mut snapshot = RegistrySnapshot::default();
        for (owner, (session, started_in)) in live.chain(retired) {
            if !started_in.starts_with(&folder) {
                continue;
            }
            snapshot.merge(&RegistrySnapshot {
                entries: vec![RegistryEntry {
                    session: *session,
                    folder: started_in.clone(),
                    standalone: Self::is_standalone_entry(inner, owner, session),
                }],
                survivors: inner
                    .survivors
                    .get(session)
                    .map(|identities| vec![(*session, identities.clone())])
                    .unwrap_or_default(),
            });
        }
        snapshot
    }

    /// A registry holding exactly `snapshot`, for a removal finished at the
    /// next start: it answers every question the same way the live run's did.
    pub fn from_snapshot(snapshot: &RegistrySnapshot) -> Self {
        let registry = Self::default();
        {
            let mut inner = registry.lock();
            for entry in &snapshot.entries {
                inner
                    .retired
                    .push_back((entry.session, entry.folder.clone()));
                if entry.standalone {
                    inner.retired_standalone.insert(entry.session);
                }
            }
            for (session, identities) in &snapshot.survivors {
                inner.survivors.insert(*session, identities.clone());
            }
        }
        registry
    }

    /// Record `identities` as members of `session` (see
    /// [`survivors_at_leader_exit`]), and into any pending removal of the
    /// folder the session runs in.
    pub fn record_survivors(&self, session: ProcessSession, identities: &[ProcessIdentity]) {
        if identities.is_empty() {
            return;
        }
        let folder = {
            let mut inner = self.lock();
            let entry = inner.survivors.entry(session).or_default();
            for identity in identities {
                if !entry.contains(identity) {
                    entry.push(*identity);
                }
            }
            inner
                .sessions
                .values()
                .flatten()
                .chain(inner.retired.iter())
                .find(|(known, _)| *known == session)
                .map(|(_, folder)| folder.clone())
        };
        if let Some(folder) = folder {
            self.sync_pending(&folder);
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
            // Noted before the thread starts, on the thread that saw the exit,
            // so a removal dispatched after this point waits for the record.
            registry.begin_recording(session);
            let worker = registry.clone();
            let spawned = std::thread::Builder::new()
                .name("pty-leader-exit".to_string())
                .spawn(move || {
                    let found = survivors_at_leader_exit(session);
                    worker.record_survivors(session, &found);
                    worker.end_recording(session);
                });
            if let Err(err) = spawned {
                registry.end_recording(session);
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
        // The agent's startup command, if one is running, is a process dux
        // started for it: it goes with the agent, whichever way it is
        // deleted. A run that registers its session only after this mark ends
        // it itself (see `StartupRunGuard::register_session`).
        if let Some(startup) = inner.startup_sessions.get(agent_id).copied() {
            let stop = inner.startup_stop.clone();
            let registry = self.clone();
            let _ = std::thread::Builder::new()
                .name("startup-command-stop".to_string())
                .spawn(move || registry.end_startup_session(startup, stop));
        }
        let mine = inner.sessions.remove(agent_id).unwrap_or_default();
        let mine: Vec<(ProcessSession, std::path::PathBuf)> = mine
            .into_iter()
            .filter(|(session, _)| {
                !inner.sessions.values().flatten().any(|(other, _)| {
                    other.sid == session.sid && other.started_at > session.started_at
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
        let folders: Vec<std::path::PathBuf> =
            mine.iter().map(|(_, folder)| folder.clone()).collect();
        drop(inner);
        for folder in folders {
            self.sync_pending(&folder);
        }
        self.enforce_caps();
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

    /// Who is told when a deleted agent's startup command would not stop,
    /// and the grace such a command is given.
    pub fn on_startup_stop_failure(
        &self,
        grace: Duration,
        notice: impl Fn(String) + Send + Sync + 'static,
    ) {
        self.lock().startup_stop = Some((grace, Arc::new(notice)));
    }

    /// End a deleted agent's startup command and what it started. Blocking:
    /// a worker thread's call.
    fn end_startup_session(
        &self,
        startup: ProcessSession,
        stop: Option<(Duration, StartupStopNotice)>,
    ) {
        let grace = stop.as_ref().map_or(KILL_SETTLE, |(grace, _)| *grace);
        let known = self.survivors_of(&[startup]);
        if let PurgeOutcome::Survivors(left) =
            purge(&mut SystemProcesses, &[startup], &known, grace)
        {
            let sentence = format!(
                "A deleted agent's startup command would not stop: {} still running. Stop it \
                 yourself.",
                describe(&left)
            );
            crate::logger::warn(&sentence);
            if let Some((_, notice)) = stop {
                notice(sentence);
            }
        }
    }

    /// The session of `agent_id`'s startup command, while it runs.
    pub fn startup_session_of(&self, agent_id: &str) -> Option<ProcessSession> {
        self.lock().startup_sessions.get(agent_id).copied()
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
    /// Register the run's session. When the agent was deleted before this
    /// point (the mark is checked under the same lock the delete sets it
    /// under), the session is ended at once instead, and `true` says so.
    /// Blocking in that case: the run's own worker thread.
    pub fn register_session(&self, session: ProcessSession, folder: &std::path::Path) -> bool {
        self.registry.register(&self.agent_id, session, folder);
        let (deleted, stop) = {
            let mut inner = self.registry.lock();
            inner
                .startup_sessions
                .insert(self.agent_id.clone(), session);
            (
                inner
                    .startup_runs
                    .get(&self.agent_id)
                    .copied()
                    .unwrap_or(false),
                inner.startup_stop.clone(),
            )
        };
        if deleted {
            self.registry.end_startup_session(session, stop);
        }
        deleted
    }

    /// What to call the run's processes in a sentence.
    pub fn label(&self, session: ProcessSession, label: &str) {
        self.registry.label(session, label);
    }

    /// Record what the run's session still has running now that its leader,
    /// the command, has exited. Blocking: the run's own worker thread.
    pub fn record_survivors(&self, session: ProcessSession) {
        let found = survivors_at_leader_exit(session);
        self.registry.record_survivors(session, &found);
    }

    /// Run `prepare` (creating the run's log folder) unless the agent has
    /// been deleted, under the same lock the delete marks it under, so a
    /// delete either comes first and nothing is created, or comes after and
    /// its own log removal takes what was created. `None` when deleted.
    pub fn unless_deleted<T>(&self, prepare: impl FnOnce() -> T) -> Option<T> {
        let inner = self.registry.lock();
        if inner
            .startup_runs
            .get(&self.agent_id)
            .copied()
            .unwrap_or(false)
        {
            return None;
        }
        let prepared = prepare();
        drop(inner);
        Some(prepared)
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
        inner.startup_sessions.remove(&self.agent_id);
        let deleted = inner.startup_runs.remove(&self.agent_id).unwrap_or(false);
        if deleted {
            // The agent's own forget already ran; a session registered after
            // it moves to the retired list, still findable by its folder.
            let late = inner.sessions.remove(&self.agent_id).unwrap_or_default();
            let folders: Vec<std::path::PathBuf> =
                late.iter().map(|(_, folder)| folder.clone()).collect();
            inner.retired.extend(late);
            drop(inner);
            for folder in folders {
                self.registry.sync_pending(&folder);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second on the boot-relative clock start times are kept in.
    const SEC: u64 = 1_000_000_000;

    fn row(pid: u32, ppid: u32, sid: u32) -> ProcRow {
        ProcRow {
            pid,
            ppid: Some(ppid),
            sid: Some(sid),
            start_time: 1_000 * SEC,
            name: format!("p{pid}"),
            exited: false,
        }
    }

    fn session(sid: u32) -> ProcessSession {
        ProcessSession {
            sid,
            started_at: 1_000 * SEC,
            boot: current_boot(),
        }
    }

    fn pids(rows: &[ProcRow]) -> Vec<u32> {
        rows.iter().map(|r| r.pid).collect()
    }

    #[test]
    fn a_stat_line_is_read_from_after_the_last_parenthesis() {
        // A command name may hold spaces and parentheses of its own.
        let stat = "4242 (my (odd) name) S 1 4242 4241 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 \
                    12345 0 0";
        let row = parse_linux_stat_with(4242, stat, 100).unwrap();
        assert_eq!(row.name, "my (odd) name");
        assert_eq!(row.ppid, Some(1));
        assert_eq!(row.sid, Some(4241));
        assert_eq!(
            row.start_time, 123_450_000_000,
            "12345 ticks at 100 per second"
        );
        assert!(!row.exited);
        let zombie =
            parse_linux_stat_with(7, "7 (z) Z 1 7 7 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 9 0", 100)
                .unwrap();
        assert!(zombie.exited);
    }

    /// A session's recorded start and the process table's are read off the
    /// same boot-relative clock, so the leader is recognised exactly, whatever
    /// the wall clock does in between.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_session_recorded_now_is_led_by_its_own_leader() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let recorded = ProcessSession::started_now(pid);
        let start = process_start(pid).unwrap();
        let table = read_process_table();
        let row = table.iter().find(|row| row.pid == pid).cloned();
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            recorded.started_at, start,
            "recorded from the kernel's own start"
        );
        assert_eq!(row.map(|row| row.start_time), Some(start));
        assert!(start <= boot_nanos_now());
    }

    /// The cap drops only sessions confirmed ended: one with a live process
    /// is kept however many newer ones there are.
    #[test]
    fn the_cap_forgets_only_sessions_that_have_ended() {
        let registry = AgentProcessRegistry::default();
        let folder = std::path::PathBuf::from("/tmp/dux-cap-test");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let live = ProcessIdentity {
            pid: child.id(),
            start_time: process_start(child.id()).unwrap_or(0),
            boot: current_boot(),
        };
        let first = ProcessSession {
            sid: 5_000_000,
            started_at: SEC,
            boot: current_boot(),
        };
        registry.register("a", first, &folder);
        registry.record_survivors(first, &[live]);
        for n in 1..=SESSIONS_PER_AGENT as u32 + 10 {
            registry.register(
                "a",
                ProcessSession {
                    sid: 5_000_000 + n,
                    started_at: SEC,
                    boot: current_boot(),
                },
                &folder,
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while registry.sessions_of("a").len() > SESSIONS_PER_AGENT && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = registry.sessions_of("a");
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            left.len() <= SESSIONS_PER_AGENT,
            "back under the cap: {}",
            left.len()
        );
        assert!(left.contains(&first), "the live one stayed");
    }

    /// A process whose folder cannot be read (a `sudo` child) is judged by
    /// the nearest readable ancestor in its session: the shell that ran it.
    #[test]
    fn an_unreadable_folder_is_judged_by_the_shell_that_ran_it() {
        let folder = crate::worktree_ops::path_key(std::path::Path::new("/work/wt"));
        let shell = row(100, 1, 100);
        let sudo = row(101, 100, 100);
        let table = vec![shell.clone(), sudo.clone()];
        let running = table.clone();
        let mut report = crate::file_drop::CwdReport::default();
        report.unknown.push(101);
        // The shell stands elsewhere: the sudo child is not in the way.
        report
            .found
            .insert(100, std::path::PathBuf::from("/home/me"));
        assert_eq!(
            judge_cwds(
                &folder,
                &running,
                &table,
                &report,
                std::path::Path::new("/repo")
            ),
            CwdVerdict::Clear
        );
        // The shell stands in the worktree: so does what it ran.
        report
            .found
            .insert(100, std::path::PathBuf::from("/work/wt/src"));
        assert_eq!(
            judge_cwds(
                &folder,
                &running,
                &table,
                &report,
                std::path::Path::new("/repo")
            ),
            CwdVerdict::Inside(&running[0])
        );
    }

    /// With nothing readable above it in its session, an unreadable process
    /// fails closed only when its session was started in or under the folder.
    #[test]
    fn an_unreadable_folder_with_no_readable_ancestor_fails_closed_only_for_its_own_folder() {
        let folder = crate::worktree_ops::path_key(std::path::Path::new("/work/wt"));
        let lone = row(200, 1, 200);
        let table = vec![lone.clone()];
        let running = table.clone();
        let mut report = crate::file_drop::CwdReport::default();
        report.unknown.push(200);
        assert_eq!(
            judge_cwds(
                &folder,
                &running,
                &table,
                &report,
                std::path::Path::new("/elsewhere")
            ),
            CwdVerdict::Clear,
            "a session started elsewhere does not block this folder"
        );
        assert_eq!(
            judge_cwds(
                &folder,
                &running,
                &table,
                &report,
                std::path::Path::new("/work/wt/x")
            ),
            CwdVerdict::Unknown(&running[0]),
            "a session started inside it may still be standing there"
        );
    }

    /// A startup run whose agent was deleted before it registered its
    /// session ends that session at once.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_session_registered_after_the_delete_is_ended_at_once() {
        use std::os::unix::process::CommandExt;
        let registry = AgentProcessRegistry::default();
        let guard = registry.begin_startup_run("agent").unwrap();
        let _ = registry.forget_agent("agent");
        let mut command = std::process::Command::new("sleep");
        command.arg("30");
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let session = ProcessSession::started_now(child.id());
        let ended = guard.register_session(session, std::path::Path::new("/tmp"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut gone = false;
        while Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(ended, "the registration said it ended the session");
        assert!(gone, "the deleted agent's startup command was ended");
    }

    /// A zombie leader still leads: its pid is allocated until it is reaped,
    /// so the session number cannot have been reused, and a job holding
    /// the leader's output (which keeps it unreaped) is still a member.
    #[test]
    fn a_zombie_leader_still_leads_its_session() {
        let mut leader = row(100, 1, 100);
        leader.exited = true;
        let table = vec![leader, row(101, 1, 100)];
        assert_eq!(pids(&members(&table, &[session(100)], &[], 9)), vec![101]);
    }

    /// A start time the platform would not give fails closed: such a process
    /// is a member through its session or its parent chain.
    #[test]
    fn an_unknown_start_time_counts_as_a_member() {
        let mut theirs = row(101, 1, 100);
        theirs.start_time = UNKNOWN_START;
        let mut their_child = row(102, 101, 102);
        their_child.start_time = UNKNOWN_START;
        let mut leader = row(100, 1, 100);
        leader.start_time = UNKNOWN_START;
        let table = vec![leader.clone(), theirs.clone(), their_child.clone()];
        assert_eq!(
            pids(&members(&table, &[session(100)], &[], 9)),
            vec![100, 101, 102],
            "an unreadable leader, member and descendant all count"
        );
        // A session whose leader is gone is still trusted only through what
        // was recorded: an unknown start time does not make a stranger ours.
        let table = vec![theirs, their_child];
        assert!(members(&table, &[session(100)], &[], 9).is_empty());
    }

    /// A session registered while the prune is reading the process table is
    /// missing from that table because it did not exist yet: it is kept.
    #[test]
    fn the_prune_keeps_a_session_registered_after_its_table_was_read() {
        let registry = AgentProcessRegistry::default();
        let folder = std::path::PathBuf::from("/tmp/dux-prune-test");
        let old = ProcessSession {
            sid: 4_000_001,
            started_at: SEC,
            boot: current_boot(),
        };
        let fresh = ProcessSession {
            sid: 4_000_002,
            started_at: 2 * SEC,
            boot: current_boot(),
        };
        registry.register("a", old, &folder);
        registry.prune_gone_with(|| {
            registry.register("b", fresh, &folder);
            Vec::new()
        });
        let left = registry.sessions_in(&folder);
        assert_eq!(
            left,
            vec![fresh],
            "the old session is gone, the fresh one kept"
        );
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
        leader.start_time = 5_000 * SEC;
        let table = vec![leader, row(101, 100, 100)];
        assert!(members(&table, &[session(100)], &[], 9).is_empty());
    }

    #[test]
    fn a_known_process_is_found_after_its_parent_link_is_cut() {
        let table = vec![row(102, 1, 102), row(103, 102, 102)];
        let known = [ProcessIdentity {
            pid: 102,
            start_time: 1_000 * SEC,
            boot: current_boot(),
        }];
        assert_eq!(
            pids(&members(&table, &[session(100)], &known, 9)),
            vec![102, 103]
        );
        // The same pid with another start time is a different process.
        let stale = [ProcessIdentity {
            pid: 102,
            start_time: 999 * SEC,
            boot: current_boot(),
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
        before.start_time = 900 * SEC;
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
        reused.start_time = 5_000 * SEC;
        let mine = row(101, 1, 100);
        let child_of_mine = row(102, 101, 100);
        let mut older_than_its_parent = row(103, 101, 100);
        older_than_its_parent.start_time = 500 * SEC;
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
            started_at: 2_000 * SEC,
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
            started_at: 1_000 * SEC,
            boot: current_boot(),
        };
        let newer = ProcessSession {
            sid: 100,
            started_at: 2_000 * SEC,
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

    /// review9: a removal must keep the worktree when the registry's record
    /// could not be saved before git runs (`record_not_saved`, whose sentence
    /// names "the session database may be busy"). A busy database makes the
    /// writer's write FAIL after SQLite's busy timeout, and the writer then
    /// counts the change as done, so `flush` answers "saved" and git runs
    /// with nothing written.
    #[test]
    fn review9_flush_does_not_report_saved_when_the_database_was_busy() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        drop(crate::storage::SessionStore::open(&db).unwrap());
        // Another connection holds the write lock for longer than the busy
        // timeout (a long transaction elsewhere, another process).
        let blocker = rusqlite::Connection::open(&db).unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        let folder = tmp.path().join("wt");
        std::fs::create_dir_all(&folder).unwrap();
        let registry = AgentProcessRegistry::default();
        registry.watch_pending("agent-1", &folder, &db);
        registry.register("agent-1", session(4242), &folder);
        let saved = registry.flush(Duration::from_secs(20));
        blocker.execute_batch("ROLLBACK;").unwrap();
        let stored = crate::storage::SessionStore::open(&db)
            .unwrap()
            .load_process_registry()
            .unwrap();
        assert!(
            stored.iter().any(|entry| entry.session == session(4242)) || !saved,
            "flush reported the record saved ({saved}) while nothing reached the database: \
             {stored:?}"
        );
    }

    /// review9: the persisted registry entry carries the session's label, so a
    /// refusal after a restart still names WHAT is standing in the folder
    /// (here a project terminal), not "a process dux started".
    #[cfg(target_os = "linux")]
    #[test]
    fn review9_a_restart_keeps_what_each_session_is_called() {
        use std::os::unix::process::CommandExt;
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        drop(crate::storage::SessionStore::open(&db).unwrap());
        let folder = tmp.path().join("wt");
        std::fs::create_dir_all(&folder).unwrap();
        let mut command = std::process::Command::new("sleep");
        command.arg("30").current_dir(&folder);
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let session = ProcessSession::started_now(child.id());
        let first = AgentProcessRegistry::default();
        first.attach_store(&db);
        first.register(UNOWNED_PTYS, session, tmp.path());
        first.label(session, "project terminal \"Terminal 2\" of project \"p\"");
        assert!(first.flush(Duration::from_secs(10)));
        let live_run = first.cwd_occupant(&folder, &[]).unwrap_or_default();
        // The next start of dux.
        let second = AgentProcessRegistry::default();
        second.attach_store(&db);
        let after_restart = second.cwd_occupant(&folder, &[]).unwrap_or_default();
        let _ = child.kill();
        let _ = child.wait();
        assert!(live_run.contains("Terminal 2"), "{live_run}");
        assert!(
            after_restart.contains("Terminal 2"),
            "after a restart the refusal no longer says what is in the folder: {after_restart}"
        );
    }
}
