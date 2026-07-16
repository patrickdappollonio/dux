//! Resource statistics for the resource-monitor overlay. Uses `sysinfo` to
//! sample CPU/RSS per process tree.
//!
//! ## Why this is a stateful collector, not a free function
//!
//! `sysinfo` derives per-process CPU from the DELTA between two refreshes of the
//! same `System`. A freshly built `System` has no baseline, so the first refresh
//! reports exactly `0.0` for every process. The original free function built a
//! `System::new()` and refreshed once per call, which is why the resource monitor
//! reported 0% CPU for everything: there was never a second refresh to diff
//! against. [`ResourceCollector`] keeps the `System` alive across samples so each
//! sample's delta spans the caller's natural poll interval (one process walk per
//! sample), and self-baselines when it has no usable previous refresh.
//!
//! Callers must sample from a background thread: a self-baselining sample sleeps
//! for [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`]. The TUI samples from its
//! resource-stats worker thread; the web samples inside `spawn_blocking`.

use std::time::{Duration, Instant};

use crate::worker::{ProcessInfo, ResourceStats};
pub use crate::worker::{ResourceKind, ResourceTarget};

/// How old the previous refresh may be before a sample re-baselines instead of
/// diffing against it. Both surfaces poll at roughly one second, so a gap beyond
/// this means the monitor was closed and reopened: diffing against a minutes-old
/// refresh would report the average CPU since then (near zero for a bursty
/// agent), which is worse than paying for a fresh baseline.
const STALE_BASELINE: Duration = Duration::from_secs(5);

/// Samples CPU/RSS for dux and a set of target process trees, holding the
/// `sysinfo::System` across calls so CPU deltas are real. See the module docs.
pub struct ResourceCollector {
    sys: sysinfo::System,
    /// When the last refresh happened, or `None` before the first sample. Drives
    /// the self-baselining decision (see [`STALE_BASELINE`]).
    last_refresh: Option<Instant>,
}

impl Default for ResourceCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceCollector {
    /// Build a collector with no baseline. Construction is cheap: no process walk
    /// happens until the first [`sample`](Self::sample).
    pub fn new() -> Self {
        Self {
            sys: sysinfo::System::new(),
            last_refresh: None,
        }
    }

    /// One process walk, refreshing CPU and memory for every process.
    fn refresh(&mut self) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};

        let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
        self.sys
            .refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);
        self.last_refresh = Some(Instant::now());
    }

    /// Sample CPU and memory for dux itself plus each labeled target process
    /// tree, newest reading wins.
    ///
    /// Blocks for [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] when it has to
    /// establish a baseline (the first sample, or the first after a gap longer
    /// than [`STALE_BASELINE`]); steady-state samples cost a single walk. Call it
    /// from a background thread.
    ///
    /// Returns `(rows, was_baseline)`. `was_baseline` is `true` exactly when
    /// THIS sample had to re-establish its CPU baseline, meaning its reading
    /// spans only the short `MINIMUM_CPU_UPDATE_INTERVAL` window rather than
    /// the caller's normal poll interval: real numbers, just noisier because
    /// they cover less wall-clock time. Callers that surface a "this reading
    /// is a short-window sample" marker to the user (the TUI's `~` prefix)
    /// should show it exactly when this is `true`, not merely on the first
    /// sample delivered to that UI session: a monitor closed and reopened
    /// inside [`STALE_BASELINE`] does NOT re-baseline, so a UI-session-scoped
    /// "first sample" flag would mark a normal steady-state reading as short-
    /// window when it is not.
    pub fn sample(&mut self, targets: Vec<ResourceTarget>) -> (Vec<ResourceStats>, bool) {
        use sysinfo::Pid;

        let needs_baseline = match self.last_refresh {
            None => true,
            Some(at) => at.elapsed() > STALE_BASELINE,
        };
        if needs_baseline {
            // Establish the "before" side of the delta, then let enough time pass
            // for sysinfo to compute a meaningful one.
            self.refresh();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        }
        self.refresh();

        let sys = &self.sys;
        let mut rows = Vec::new();

        // Row: dux itself.
        let self_pid = Pid::from_u32(std::process::id());
        if let Some(proc_info) = sys.process(self_pid) {
            rows.push(ResourceStats {
                id: None,
                kind: ResourceKind::Dux,
                label: "dux (this process)".into(),
                pid: Some(std::process::id()),
                cpu_percent: proc_info.cpu_usage(),
                rss_bytes: proc_info.memory(),
                process_count: 1,
                children: Vec::new(),
            });
        }

        // Rows: each labeled target (agent tabs and companion terminals). The
        // target's identity is carried through verbatim so the caller can join
        // the row back to its spine entity by id.
        for target in &targets {
            let (cpu, rss, count, children) = aggregate_tree(sys, Pid::from_u32(target.pid));
            rows.push(ResourceStats {
                id: Some(target.id.clone()),
                kind: target.kind,
                label: target.label.clone(),
                pid: Some(target.pid),
                cpu_percent: cpu,
                rss_bytes: rss,
                process_count: count,
                children,
            });
        }

        // Total row. CPU legitimately exceeds 100% for a multi-threaded tree
        // spread across cores, so this sum is deliberately not clamped.
        let total_cpu: f32 = rows.iter().map(|r| r.cpu_percent).sum();
        let total_rss: u64 = rows.iter().map(|r| r.rss_bytes).sum();
        let total_procs: usize = rows.iter().map(|r| r.process_count).sum();
        rows.push(ResourceStats {
            id: None,
            kind: ResourceKind::Total,
            label: "TOTAL".into(),
            pid: None,
            cpu_percent: total_cpu,
            rss_bytes: total_rss,
            process_count: total_procs,
            children: Vec::new(),
        });

        (rows, needs_baseline)
    }
}

/// Check whether `pid` is a descendant (child, grandchild, ...) of `ancestor`
/// by walking up the process tree.
fn is_descendant_of(sys: &sysinfo::System, pid: sysinfo::Pid, ancestor: sysinfo::Pid) -> bool {
    use sysinfo::Pid;

    let mut current = pid;
    // Depth limit prevents infinite loops if the tree has a cycle (shouldn't
    // happen, but be defensive).
    for _ in 0..64 {
        if let Some(proc) = sys.process(current) {
            if let Some(parent) = proc.parent() {
                if parent == ancestor {
                    return true;
                }
                if parent == Pid::from_u32(0) {
                    return false;
                }
                current = parent;
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    false
}

/// Aggregate CPU% and RSS across a root PID and all its descendants.
/// Returns `(total_cpu, total_rss, process_count, top_children)` where
/// `top_children` contains the top 10 individual processes by RSS.
fn aggregate_tree(
    sys: &sysinfo::System,
    root: sysinfo::Pid,
) -> (f32, u64, usize, Vec<ProcessInfo>) {
    let mut cpu = 0.0f32;
    let mut rss = 0u64;
    let mut count = 0usize;
    let mut children = Vec::new();
    for (pid, proc_info) in sys.processes() {
        // On Linux, `sysinfo` lists userland/kernel THREADS alongside real
        // processes in `sys.processes()`. A thread shares its process's
        // address space, so `Process::memory()` on a thread entry reports the
        // WHOLE process's RSS, and `thread_kind()` is `Some(_)` only for
        // these thread entries (real processes report `None`). Without this
        // filter, every thread of the root (or of any descendant) gets
        // counted as a distinct "child" with duplicated RSS, which both
        // inflates `rss`/`process_count` and lists fake subprocesses in the
        // UI. A process's own `cpu_usage()` already aggregates all its
        // threads' CPU on Linux, so skipping thread entries here also avoids
        // double-counting CPU, not just memory.
        if proc_info.thread_kind().is_some() {
            continue;
        }
        if *pid == root || is_descendant_of(sys, *pid, root) {
            cpu += proc_info.cpu_usage();
            rss += proc_info.memory();
            count += 1;
            children.push(ProcessInfo {
                name: proc_info.name().to_string_lossy().into_owned(),
                pid: pid.as_u32(),
                cpu_percent: proc_info.cpu_usage(),
                rss_bytes: proc_info.memory(),
            });
        }
    }
    children.sort_by_key(|b| std::cmp::Reverse(b.rss_bytes));
    children.truncate(10);
    (cpu, rss, count, children)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Spin a thread burning CPU until the returned flag is set. The join handle
    /// must be awaited by the caller so the thread never outlives the test.
    fn burn_a_core() -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut x: u64 = 0;
            while !flag.load(Ordering::Relaxed) {
                // Opaque to the optimizer so the loop is not elided.
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
        (stop, handle)
    }

    /// The regression guard for the always-zero-CPU bug: `sysinfo` derives
    /// per-process CPU from the delta between two refreshes, so a collector with
    /// no baseline reports exactly 0.0 forever. A real load must read above zero
    /// on the FIRST sample (the collector self-baselines) and on later ones.
    #[test]
    fn collector_reports_nonzero_cpu_under_load() {
        let (stop, handle) = burn_a_core();

        let mut collector = ResourceCollector::new();
        let (first, first_was_baseline) = collector.sample(Vec::new());
        let first_cpu = first
            .iter()
            .find(|r| r.label == "dux (this process)")
            .expect("the dux row is always present")
            .cpu_percent;

        std::thread::sleep(Duration::from_millis(300));
        let (second, second_was_baseline) = collector.sample(Vec::new());
        let second_cpu = second
            .iter()
            .find(|r| r.label == "dux (this process)")
            .expect("the dux row is always present")
            .cpu_percent;

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        assert!(
            first_cpu > 0.0,
            "the first sample must self-baseline and report real CPU, got {first_cpu}"
        );
        assert!(
            second_cpu > 0.0,
            "a steady-state sample must report real CPU, got {second_cpu}"
        );
        assert!(first_was_baseline, "the first sample always re-baselines");
        assert!(
            !second_was_baseline,
            "a steady-state sample within STALE_BASELINE must not re-baseline"
        );
    }

    /// `was_baseline` must reflect what THIS sample actually did, not a
    /// UI-session-scoped guess: true on the very first sample, false on an
    /// immediate follow-up, and true again after a gap longer than
    /// `STALE_BASELINE` with no sampling in between (the monitor was closed
    /// and reopened later). This is the fact the TUI's `~` short-window
    /// marker is built on (finding 3).
    #[test]
    fn sample_reports_was_baseline_accurately() {
        let mut collector = ResourceCollector::new();

        let (_, first_was_baseline) = collector.sample(Vec::new());
        assert!(
            first_was_baseline,
            "the very first sample has no prior refresh and must baseline"
        );

        let (_, second_was_baseline) = collector.sample(Vec::new());
        assert!(
            !second_was_baseline,
            "a sample taken immediately after must not re-baseline"
        );

        // Simulate the monitor being closed and reopened after a gap longer
        // than STALE_BASELINE, with no sampling in between.
        collector.last_refresh = Some(Instant::now() - STALE_BASELINE - Duration::from_millis(1));
        let (_, third_was_baseline) = collector.sample(Vec::new());
        assert!(
            third_was_baseline,
            "a sample after a stale gap must re-baseline"
        );
    }

    /// A busy multi-threaded tree legitimately exceeds 100% across cores, so
    /// nothing in the collector may clamp the value.
    #[test]
    fn collector_does_not_clamp_cpu_at_one_hundred_percent() {
        // Two burners plus the test thread; on any multi-core box this exceeds
        // 100% for the process. Skip the assertion on a single-core machine
        // rather than fail it spuriously.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        if cores < 4 {
            return;
        }
        let burners: Vec<_> = (0..3).map(|_| burn_a_core()).collect();

        let mut collector = ResourceCollector::new();
        let _ = collector.sample(Vec::new());
        std::thread::sleep(Duration::from_millis(400));
        let (rows, _) = collector.sample(Vec::new());
        let cpu = rows
            .iter()
            .find(|r| r.label == "dux (this process)")
            .expect("the dux row is always present")
            .cpu_percent;

        for (stop, handle) in burners {
            stop.store(true, Ordering::Relaxed);
            handle.join().unwrap();
        }

        assert!(
            cpu > 100.0,
            "three busy threads should read above 100% on a {cores}-core box; \
             got {cpu} (a clamp would pin this at exactly 100)"
        );
    }

    /// Re-sampling immediately (inside `MINIMUM_CPU_UPDATE_INTERVAL`) must not
    /// zero the reading: sysinfo keeps the previous delta rather than
    /// recomputing, and the collector must not fabricate a zero either.
    #[test]
    fn back_to_back_samples_do_not_zero_cpu() {
        let (stop, handle) = burn_a_core();

        let mut collector = ResourceCollector::new();
        let _ = collector.sample(Vec::new());
        std::thread::sleep(Duration::from_millis(300));
        let _ = collector.sample(Vec::new());
        // Immediately again, with no sleep at all.
        let (rows, _) = collector.sample(Vec::new());
        let cpu = rows
            .iter()
            .find(|r| r.label == "dux (this process)")
            .expect("the dux row is always present")
            .cpu_percent;

        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();

        assert!(
            cpu > 0.0,
            "a back-to-back sample must retain the last real reading, got {cpu}"
        );
    }

    #[test]
    fn current_process_is_descendant_of_pid_1() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let self_pid = Pid::from_u32(std::process::id());
        let init_pid = Pid::from_u32(1);
        assert!(
            is_descendant_of(&sys, self_pid, init_pid),
            "current process should be a descendant of PID 1"
        );
    }

    #[test]
    fn aggregate_tree_includes_self_process() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let self_pid = Pid::from_u32(std::process::id());
        let (_cpu, rss, count, _children) = aggregate_tree(&sys, self_pid);
        assert!(count >= 1, "should include at least the root process");
        assert!(rss > 0, "current process should have nonzero RSS");
    }

    /// Serializes the tests below that spawn real child processes
    /// (`sleep`/`sh`). All tests in this binary share one OS process, so a
    /// child spawned by one test is, for that instant, also a real child of
    /// every other test running concurrently in the same binary: without
    /// this lock, `aggregate_tree(self_pid)` in one test would pick up
    /// another test's still-running `sleep` and inflate `process_count`,
    /// which is exactly the kind of cross-test flake that looks like a
    /// regression but is a test isolation bug, not a code bug.
    static CHILD_PROCESS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Spin `n` extra userland threads inside the current process and return
    /// their stop flag and join handles. The caller must stop and join them
    /// before the test ends.
    fn spin_threads(n: usize) -> (Arc<AtomicBool>, Vec<std::thread::JoinHandle<()>>) {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = (0..n)
            .map(|_| {
                let flag = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !flag.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                })
            })
            .collect();
        (stop, handles)
    }

    fn fresh_system() -> sysinfo::System {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
        let mut sys = System::new();
        // Establish a CPU baseline, then refresh again so cpu_usage() is real.
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        sys
    }

    /// Threads share their process's address space, so on Linux `sysinfo`
    /// lists each userland thread in `sys.processes()` reporting the WHOLE
    /// process's RSS. Without filtering `thread_kind().is_some()` entries,
    /// `aggregate_tree` lists these threads as fake "children" and sums their
    /// duplicated RSS into the parent total. This test proves the fix: no
    /// child entry duplicates the root process's own RSS, and the real
    /// `sleep` child process is still present.
    #[test]
    fn aggregate_tree_excludes_threads_from_children() {
        let _guard = CHILD_PROCESS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (stop, handles) = spin_threads(4);

        let mut child = std::process::Command::new("sleep")
            .arg("3")
            .spawn()
            .expect("failed to spawn sleep child");
        let child_pid = child.id();

        // Let sysinfo see the new child and threads.
        std::thread::sleep(Duration::from_millis(200));
        let sys = fresh_system();

        let self_pid = sysinfo::Pid::from_u32(std::process::id());
        let self_rss = sys
            .process(self_pid)
            .expect("current process must be visible")
            .memory();

        let (_cpu, _rss, _count, children) = aggregate_tree(&sys, self_pid);

        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        let _ = child.kill();
        let _ = child.wait();

        let duplicate_count = children
            .iter()
            .filter(|c| c.pid != self_pid.as_u32() && c.rss_bytes == self_rss && self_rss > 0)
            .count();
        assert_eq!(
            duplicate_count, 0,
            "no child should duplicate the root process's own RSS (threads masquerading as children): {children:?}"
        );

        assert!(
            children.iter().any(|c| c.pid == child_pid),
            "the real sleep child process must be present in children: {children:?}"
        );
    }

    /// With N extra threads and one real child process, the aggregated RSS
    /// must not be inflated by summing each thread's duplicated whole-process
    /// RSS on top of the process's own RSS. Assert a loose upper bound rather
    /// than an exact byte value to avoid flakiness.
    #[test]
    fn aggregate_tree_does_not_multiply_count_thread_memory() {
        let _guard = CHILD_PROCESS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (stop, handles) = spin_threads(5);

        let mut child = std::process::Command::new("sleep")
            .arg("3")
            .spawn()
            .expect("failed to spawn sleep child");

        std::thread::sleep(Duration::from_millis(200));
        let sys = fresh_system();

        let self_pid = sysinfo::Pid::from_u32(std::process::id());
        let self_rss = sys
            .process(self_pid)
            .expect("current process must be visible")
            .memory();

        let (_cpu, rss, _count, _children) = aggregate_tree(&sys, self_pid);

        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            rss < self_rss * 2,
            "aggregated rss ({rss}) must be close to self ({self_rss}) plus the \
             sleep child, not inflated by summing duplicated thread memory"
        );
    }

    /// `process_count` must count real processes only: self plus the one
    /// spawned child, never the extra threads spun up inside this process.
    #[test]
    fn aggregate_tree_counts_processes_not_threads() {
        let _guard = CHILD_PROCESS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (stop, handles) = spin_threads(3);

        let mut child = std::process::Command::new("sleep")
            .arg("3")
            .spawn()
            .expect("failed to spawn sleep child");

        std::thread::sleep(Duration::from_millis(200));
        let sys = fresh_system();

        let self_pid = sysinfo::Pid::from_u32(std::process::id());
        let (_cpu, _rss, count, _children) = aggregate_tree(&sys, self_pid);

        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(
            count, 2,
            "process_count must be self + the one real child, not threads"
        );
    }

    /// A real grandchild process (spawned by an intermediate `sh -c`) must
    /// still be discovered as a descendant even though threads are filtered
    /// out of the walk, proving the thread filter did not break descendant
    /// discovery through a threaded intermediate parent.
    #[test]
    fn aggregate_tree_finds_real_grandchild_process() {
        let _guard = CHILD_PROCESS_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (stop, handles) = spin_threads(2);

        // `sleep 3 &` backgrounds sleep as a real forked child of the `sh`
        // process (not an exec-replace), guaranteeing a genuine grandchild of
        // the test process; `wait` keeps `sh` alive until sleep exits.
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg("sleep 3 & wait")
            .spawn()
            .expect("failed to spawn sh child");

        std::thread::sleep(Duration::from_millis(300));
        let sys = fresh_system();

        let self_pid = sysinfo::Pid::from_u32(std::process::id());
        let (_cpu, _rss, count, children) = aggregate_tree(&sys, self_pid);

        // Find the grandchild: a process whose parent is the `sh` child.
        let sh_pid = sysinfo::Pid::from_u32(child.id());
        let grandchild = sys
            .processes()
            .iter()
            .find(|(pid, proc_info)| **pid != sh_pid && proc_info.parent() == Some(sh_pid));

        stop.store(true, Ordering::Relaxed);
        for h in handles {
            h.join().unwrap();
        }
        let _ = child.kill();
        let _ = child.wait();

        let (grandchild_pid, _) = grandchild.expect(
            "the sh process must have forked a real sleep grandchild; \
             if this fails the shell execed instead of forking on this platform",
        );
        assert!(
            children.iter().any(|c| c.pid == grandchild_pid.as_u32()),
            "a real grandchild process must still be aggregated through an intermediate parent: {children:?}"
        );
        assert!(
            count >= 3,
            "process_count must include self, the sh child, and the grandchild: got {count}"
        );
    }

    #[test]
    fn is_descendant_of_returns_false_for_unrelated_pid() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        // PID 1 is not a descendant of the current process.
        let self_pid = Pid::from_u32(std::process::id());
        let init_pid = Pid::from_u32(1);
        assert!(!is_descendant_of(&sys, init_pid, self_pid));
    }
}
