//! Resource statistics for the resource-monitor overlay. Uses `sysinfo` to
//! sample CPU/RSS per process tree. The `spawn_resource_stats_worker`
//! Engine method (T3b) calls `collect_resource_stats` on a worker thread.

use crate::worker::{ProcessInfo, ResourceStats};

/// Collect CPU and memory stats for dux itself plus each labeled target
/// process tree. Runs on a background thread — no `&self` needed.
pub fn collect_resource_stats(targets: Vec<(String, u32)>) -> Vec<ResourceStats> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let mut sys = System::new();
    let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    let mut rows = Vec::new();

    // Row: dux itself.
    let self_pid = Pid::from_u32(std::process::id());
    if let Some(proc_info) = sys.process(self_pid) {
        rows.push(ResourceStats {
            label: "dux (this process)".into(),
            pid: Some(std::process::id()),
            cpu_percent: proc_info.cpu_usage(),
            rss_bytes: proc_info.memory(),
            process_count: 1,
            children: Vec::new(),
        });
    }

    // Rows: each labeled target (agents and companion terminals).
    for (label, root_pid) in &targets {
        let (cpu, rss, count, children) = aggregate_tree(&sys, Pid::from_u32(*root_pid));
        rows.push(ResourceStats {
            label: label.clone(),
            pid: Some(*root_pid),
            cpu_percent: cpu,
            rss_bytes: rss,
            process_count: count,
            children,
        });
    }

    // Total row.
    let total_cpu: f32 = rows.iter().map(|r| r.cpu_percent).sum();
    let total_rss: u64 = rows.iter().map(|r| r.rss_bytes).sum();
    let total_procs: usize = rows.iter().map(|r| r.process_count).sum();
    rows.push(ResourceStats {
        label: "TOTAL".into(),
        pid: None,
        cpu_percent: total_cpu,
        rss_bytes: total_rss,
        process_count: total_procs,
        children: Vec::new(),
    });

    rows
}

/// Check whether `pid` is a descendant (child, grandchild, ...) of `ancestor`
/// by walking up the process tree.
pub fn is_descendant_of(sys: &sysinfo::System, pid: sysinfo::Pid, ancestor: sysinfo::Pid) -> bool {
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
pub fn aggregate_tree(
    sys: &sysinfo::System,
    root: sysinfo::Pid,
) -> (f32, u64, usize, Vec<ProcessInfo>) {
    let mut cpu = 0.0f32;
    let mut rss = 0u64;
    let mut count = 0usize;
    let mut children = Vec::new();
    for (pid, proc_info) in sys.processes() {
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
