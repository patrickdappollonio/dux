//! One lane per worktree for every git change dux makes in it (stage,
//! unstage, discard, commit), whichever surface asked, so they run in the
//! order they were asked for.
//!
//! Each change runs on a worker of its own, and two workers started back to
//! back can finish in either order: a stage of a file and then an unstage of it
//! could leave it staged, which contradicts the later request. A change takes a
//! ticket on the engine thread, in arrival order, and its worker waits for its
//! turn before touching git. A ticket whose worker never runs gives its turn
//! up, so the lane never wedges.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

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
    let lane = {
        let mut lanes = lanes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(lanes.entry(worktree.to_path_buf()).or_default())
    };
    let number = {
        let mut state = lane
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let number = state.next;
        state.next += 1;
        number
    };
    Ticket {
        lane,
        number,
        used: false,
    }
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
}
