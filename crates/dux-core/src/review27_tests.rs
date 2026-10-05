//! Review 27: a reaped child's group is judged against members whose record
//! may still be in flight.
use crate::process_sessions::{AgentProcessRegistry, process_start};
use crate::pty::PtyClient;
use std::time::{Duration, Instant};

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            let close = s.rfind(')').unwrap();
            !s[close + 1..].trim_start().starts_with('Z')
        })
        .unwrap_or(false)
}

fn kill(pid: u32) {
    if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
        let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
    }
}

/// Wire a client the way the engine does (events.rs / companion.rs), let its
/// leader exit leaving a job in its group, observe the exit with try_wait (as
/// the exit prune and the terminating reap do) and drop it in the same tick.
fn run(wait_for_record: bool) -> (u32, bool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let pidfile = dir.path().join("job.pid");
    let args = vec![
        "-c".to_string(),
        format!(
            "trap '' HUP; sleep 300 & echo $! > '{}'; exit 0",
            pidfile.display()
        ),
    ];
    let registry = AgentProcessRegistry::default();
    let mut client = PtyClient::spawn("/bin/sh", &args, dir.path(), 5, 40, 100).expect("spawn");
    let process = client.process_session().expect("a session");
    registry.register("agent", process, dir.path());
    client.set_leader_exit_hook(registry.leader_exit_hook(process));
    let source = registry.clone();
    client.set_recorded_members(Box::new(move || source.survivors_of(&[process])));
    let deadline = Instant::now() + Duration::from_secs(5);
    while client.try_wait().is_none() {
        assert!(Instant::now() < deadline, "the shell did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
    let job: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(process_start(job).is_some());
    if wait_for_record {
        registry.wait_for_recordings(&[process], Duration::from_secs(5));
    }
    drop(client);
    std::thread::sleep(Duration::from_millis(500));
    let survived = alive(job);
    kill(job);
    (job, survived)
}

#[test]
fn review27_control_with_the_record_landed_the_drop_kills_the_group() {
    let (job, survived) = run(true);
    assert!(
        !survived,
        "control: job {job} should have been SIGKILLed with its group"
    );
}

#[test]
fn review27_a_drop_right_after_the_reap_leaves_the_groups_job_running() {
    let (job, survived) = run(false);
    assert!(
        !survived,
        "job {job}, in the reaped child's own process group, outlived the client's drop: \
         the group was judged before the leader-exit record landed, so it was never signalled"
    );
}
