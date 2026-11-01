//! One lane per worktree for every git change dux makes in it (stage,
//! unstage, discard, commit), whichever surface asked, so they run in the
//! order they were asked for, and for dux's own reads of its changed files,
//! which git answers by writing to the same index.
//!
//! Each change runs on a worker of its own, and two workers started back to
//! back can finish in either order: a stage of a file and then an unstage of it
//! could leave it staged, which contradicts the later request. A change takes a
//! ticket on the engine thread, in arrival order, and its worker waits for its
//! turn before touching git. A ticket whose worker never runs gives its turn
//! up, so the lane never wedges.
//!
//! A listing of the changed files takes its place on the worker that reads,
//! right before it reads: `git status` and `git diff` take the worktree's
//! `index.lock` to write refreshed file times back, and a commit that starts
//! meanwhile is refused by git. A poll ([`poll_changed_files`]) never waits: it
//! reads only when the lane is idle and otherwise skips. A read somebody is
//! waiting on ([`request_changed_files`]) waits a bounded time and then reads
//! without taking the lock.
//!
//! A lane is keyed by the path it is asked for. Where a session's directory is
//! recorded, [`record_worktree`] joins that spelling to the lane of the
//! directory it resolves to, so a symlinked spelling waits in the same line.
//! Any other path (a test's, the focused-tab writes' pseudo-path) is its own
//! key as given.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::git::ListingLocks;
use crate::model::ChangedFile;

#[derive(Debug, Default)]
struct LaneState {
    /// The ticket the next change gets.
    next: u64,
    /// The ticket whose turn it is.
    serving: u64,
    /// Tickets given up before their turn came.
    skipped: BTreeSet<u64>,
}

#[derive(Debug, Default)]
struct Lane {
    state: Mutex<LaneState>,
    turn: Condvar,
}

impl Lane {
    /// Pass the turn on from `done`, past every ticket given up.
    fn advance(&self, state: &mut LaneState) {
        state.serving += 1;
        while state.skipped.remove(&state.serving) {
            state.serving += 1;
        }
        self.turn.notify_all();
    }
}

fn lanes() -> &'static Mutex<HashMap<PathBuf, Arc<Lane>>> {
    static LANES: OnceLock<Mutex<HashMap<PathBuf, Arc<Lane>>>> = OnceLock::new();
    LANES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Run `take` on the lane `worktree` is keyed to (made on first use) with its
/// state locked, all while the map is held, so [`record_worktree`] cannot
/// rekey that spelling between finding the lane and taking a place in it: a
/// place taken in a lane the moment it was replaced would run beside the turns
/// of the new one. No filesystem access: it is called on the engine and
/// interface threads.
fn take_place<T>(worktree: &Path, take: impl FnOnce(&Arc<Lane>, &mut LaneState) -> T) -> T {
    let mut lanes = lanes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let lane = lanes.entry(worktree.to_path_buf()).or_default();
    let mut state = lane
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    take(lane, &mut state)
}

impl Lane {
    /// Nobody holds a turn or waits for one.
    fn is_idle(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.serving == state.next
    }
}

/// Join `worktree`'s lane to the lane of the directory it resolves to, so a
/// symlinked spelling of a session's directory waits in the same line as the
/// resolved one. Resolving reads the filesystem, so this runs on a worker,
/// where the session's directory is recorded (created, loaded, adopted,
/// recreated); [`ticket`] and [`try_turn`] then find the joined lane with no
/// filesystem access. A path that does not resolve keeps its own lane, and two
/// spellings that already have lanes of their own are joined only while the
/// one being rekeyed is idle, because a ticket in it would lose its place.
pub fn record_worktree(worktree: &Path) {
    let Ok(resolved) = worktree.canonicalize() else {
        return;
    };
    if resolved == worktree {
        return;
    }
    let mut lanes = lanes()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let given = lanes.get(worktree).cloned();
    let canonical = lanes.get(&resolved).cloned();
    match (given, canonical) {
        (Some(given), Some(canonical)) => {
            if !Arc::ptr_eq(&given, &canonical) && given.is_idle() {
                lanes.insert(worktree.to_path_buf(), canonical);
            }
        }
        (Some(given), None) => {
            lanes.insert(resolved, given);
        }
        (None, Some(canonical)) => {
            lanes.insert(worktree.to_path_buf(), canonical);
        }
        (None, None) => {
            let lane = Arc::new(Lane::default());
            lanes.insert(worktree.to_path_buf(), Arc::clone(&lane));
            lanes.insert(resolved, lane);
        }
    }
}

/// `worktree`'s turn now, when nobody holds it or waits for it; `None`
/// otherwise. Never queues, so nothing ever waits behind it.
pub fn try_turn(worktree: &Path) -> Option<Turn> {
    take_place(worktree, |lane, state| {
        if state.serving != state.next {
            return None;
        }
        state.next += 1;
        Some(Turn {
            lane: Arc::clone(lane),
        })
    })
}

/// One listing's rows: (staged, unstaged).
pub type ChangedFiles = (Vec<ChangedFile>, Vec<ChangedFile>);

/// How long a [`request_changed_files`] waits for its turn: well inside the
/// web's own per-read bound, so a change running a long hook never leaves the
/// pane unanswered.
pub const REQUEST_TURN_WAIT: Duration = Duration::from_secs(5);

/// A periodic poll of `worktree`'s changed files: it reads only when nobody
/// holds or waits for the lane, and answers `None` without reading otherwise.
/// Its `git status` takes no lock. Call it on the worker that reads; the turn
/// is held across every git child the listing runs.
pub fn poll_changed_files(worktree: &Path) -> Option<anyhow::Result<ChangedFiles>> {
    let _turn = try_turn(worktree)?;
    Some(crate::git::changed_files_for_display_with(
        worktree,
        ListingLocks::Poll,
    ))
}

/// A read of `worktree`'s changed files somebody is waiting on (a refresh
/// after a change, a request for the list). It takes its ticket here, on the
/// worker, right before it reads, waits up to [`REQUEST_TURN_WAIT`] for its
/// turn, and past that reads once without taking the lock rather than keep
/// them waiting.
///
/// # Errors
///
/// Fails when the listing does.
pub fn request_changed_files(worktree: &Path) -> anyhow::Result<ChangedFiles> {
    request_changed_files_within(worktree, REQUEST_TURN_WAIT)
}

/// [`request_changed_files`], waiting `wait` for the turn.
///
/// # Errors
///
/// Fails when the listing does.
pub fn request_changed_files_within(
    worktree: &Path,
    wait: Duration,
) -> anyhow::Result<ChangedFiles> {
    if let Some(_turn) = ticket(worktree).wait_at_most(wait) {
        return crate::git::changed_files_for_display_with(worktree, ListingLocks::Refresh);
    }
    crate::logger::info(&format!(
        "the changed files of {} were read without the index lock: a change to it still held \
         its turn after {}s",
        worktree.display(),
        crate::duration_text::whole_seconds(wait)
    ));
    crate::git::changed_files_for_display_with(worktree, ListingLocks::LockFree)
}

/// A place in `worktree`'s lane, taken in the order changes are asked for.
#[derive(Debug)]
pub struct Ticket {
    lane: Arc<Lane>,
    number: u64,
    used: bool,
}

/// Take the next place in `worktree`'s lane. In memory only: call it where
/// the change is asked for (the engine thread, a route, the UI thread), before
/// handing it to the worker that waits its turn.
pub fn ticket(worktree: &Path) -> Ticket {
    take_place(worktree, |lane, state| {
        let number = state.next;
        state.next += 1;
        Ticket {
            lane: Arc::clone(lane),
            number,
            used: false,
        }
    })
}

/// The turn a ticket waited for; the next change's turn starts when it drops.
#[derive(Debug)]
pub struct Turn {
    lane: Arc<Lane>,
}

impl Ticket {
    /// Wait, on the worker, until every change asked for earlier has finished.
    pub fn wait(mut self) -> Turn {
        self.used = true;
        let mut state = self
            .lane
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.serving != self.number {
            state = self
                .lane
                .turn
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        drop(state);
        Turn {
            lane: Arc::clone(&self.lane),
        }
    }
}

impl Ticket {
    /// [`Self::wait`], giving up after `bound`: `None` when the turn did not
    /// come in time, in which case this ticket's turn is given up.
    pub fn wait_at_most(mut self, bound: std::time::Duration) -> Option<Turn> {
        let deadline = std::time::Instant::now() + bound;
        let mut state = self
            .lane
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while state.serving != self.number {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            state = self
                .lane
                .turn
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        drop(state);
        self.used = true;
        Some(Turn {
            lane: Arc::clone(&self.lane),
        })
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if self.used {
            return;
        }
        // Never waited for: its worker did not run, so its turn is given up.
        let mut state = self
            .lane
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.serving == self.number {
            self.lane.advance(&mut state);
        } else {
            state.skipped.insert(self.number);
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut state = self
            .lane
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.lane.advance(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A later change waits for an earlier one still running, however soon
    /// its own worker starts.
    #[test]
    fn a_later_change_waits_for_the_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = ticket(dir.path());
        let second = ticket(dir.path());
        let (release, gate) = mpsc::channel::<()>();
        let (order_tx, order) = mpsc::channel::<&str>();
        let order_second = order_tx.clone();
        let later = std::thread::spawn(move || {
            let _turn = second.wait();
            order_second.send("second").unwrap();
        });
        let earlier = std::thread::spawn(move || {
            let _turn = first.wait();
            gate.recv().unwrap();
            order_tx.send("first").unwrap();
        });
        assert!(
            order.recv_timeout(Duration::from_millis(200)).is_err(),
            "the later change ran while the earlier one held its turn"
        );
        release.send(()).unwrap();
        assert_eq!(order.recv().unwrap(), "first");
        assert_eq!(order.recv().unwrap(), "second");
        earlier.join().unwrap();
        later.join().unwrap();
    }

    /// A ticket given up (its worker never ran) does not stall the ones
    /// behind it.
    #[test]
    fn a_ticket_given_up_passes_its_turn_on() {
        let dir = tempfile::tempdir().unwrap();
        let first = ticket(dir.path());
        let abandoned = ticket(dir.path());
        let third = ticket(dir.path());
        drop(abandoned);
        drop(first.wait());
        let (done_tx, done) = mpsc::channel();
        std::thread::spawn(move || {
            let _turn = third.wait();
            done_tx.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(5))
            .expect("the turn reached the third ticket");
    }

    fn repo_with_a_change() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "user.email", "t@t"]);
        std::fs::write(dir.path().join("f.txt"), "one\n").unwrap();
        git(&["add", "f.txt"]);
        git(&["commit", "-q", "-m", "init"]);
        std::fs::write(dir.path().join("f.txt"), "two\n").unwrap();
        dir
    }

    /// A poll finds the lane busy, reads nothing and takes no place in it: the
    /// change behind the busy turn gets its turn the moment that one ends.
    #[test]
    fn a_poll_skips_a_busy_lane_and_queues_nothing() {
        let repo = repo_with_a_change();
        let running = ticket(repo.path()).wait();

        assert!(poll_changed_files(repo.path()).is_none());

        let next = ticket(repo.path());
        drop(running);
        assert!(next.wait_at_most(Duration::ZERO).is_some());
        let (_, unstaged) = poll_changed_files(repo.path())
            .expect("an idle lane is read")
            .unwrap();
        assert_eq!(unstaged.len(), 1);
    }

    /// A request that does not get its turn in time still answers, read
    /// without the lock, and gives its place up so nothing waits behind it.
    #[test]
    fn a_request_past_its_wait_reads_lock_free_and_gives_its_place_up() {
        let repo = repo_with_a_change();
        let running = ticket(repo.path()).wait();

        let (_, unstaged) =
            request_changed_files_within(repo.path(), Duration::from_millis(50)).unwrap();
        assert_eq!(unstaged[0].path, "f.txt");

        drop(running);
        assert!(
            try_turn(repo.path()).is_some(),
            "the request left a place nobody will take"
        );
    }

    /// A symlinked spelling of a recorded worktree waits in the same lane as
    /// the directory it resolves to.
    #[test]
    fn a_symlinked_spelling_shares_the_recorded_lane() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        record_worktree(&link);

        let running = ticket(&real.canonicalize().unwrap()).wait();
        assert!(try_turn(&link).is_none(), "the link has a lane of its own");
        drop(running);
        assert!(try_turn(&link).is_some());
    }

    /// A listing stopped at its deadline gives its turn back only once its git
    /// has exited, so the change waiting behind it never runs beside it.
    #[test]
    fn a_stopped_listing_holds_its_turn_until_its_git_has_exited() {
        let repo = repo_with_a_change();
        let hook = repo.path().join(".git/fsmonitor-hang");
        let pid_file = repo.path().join(".git/git-pid");
        std::fs::write(
            &hook,
            format!(
                "#!/bin/sh\ntrap '' TERM\necho $PPID > {}\nsleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let out = crate::git::test_support::git_command()
            .args(["config", "core.fsmonitor", hook.to_str().unwrap()])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(out.status.success());

        let worktree = repo.path().to_path_buf();
        let reader = std::thread::spawn(move || {
            crate::git::listing_child::with_limits(
                Duration::from_millis(300),
                Duration::from_millis(300),
                || request_changed_files(&worktree),
            )
        });
        while !pid_file.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let next = ticket(repo.path());
        let _turn = next.wait();

        let git: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let git = rustix::process::Pid::from_raw(git).unwrap();
        assert!(
            rustix::process::test_kill_process(git).is_err(),
            "the turn came back while the listing's git still ran"
        );
        assert!(reader.join().unwrap().is_err());
    }
}
