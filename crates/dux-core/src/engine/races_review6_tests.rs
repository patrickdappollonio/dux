//! Sixth adversarial review of the worktree-removal races branch.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::engine::Engine;
use crate::engine::test_support::{sample_project, sample_session, test_engine};

fn git(dir: &Path, args: &[&str]) {
    let out = crate::test_git::fixture_git()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn repo(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "t"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    repo
}

fn agent_worktree(engine: &mut Engine, root: &Path, repo: &Path, name: &str) -> PathBuf {
    let worktree = root.join("worktrees").join("p1-name").join(name);
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    git(
        repo,
        &["worktree", "add", "-b", name, worktree.to_str().unwrap()],
    );
    let mut session = sample_session(&format!("s-{name}"), "p1", name);
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = worktree.to_string_lossy().into_owned();
    }
    engine.sessions.push(session);
    worktree
}

fn cwd_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// "dux must never remove ... a folder that something it started is still
/// using, whether in that folder or anywhere inside it." A project terminal is
/// a PTY dux started; its shell stands in an agent's worktree (the user `cd`'d
/// there to run the tests). Deleting the agent with its worktree removes the
/// folder from under that live terminal: every occupancy question asks only
/// where a PTY was SPAWNED (the project root), never where its processes are.
#[test]
fn a_project_terminal_standing_in_a_worktree_keeps_it() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    engine.config.terminal.command = "sh".to_string();
    engine.config.terminal.args = vec![
        "-c".to_string(),
        format!("cd '{}' && exec sleep 60", worktree.display()),
    ];
    let (terminal_id, _) = engine
        .create_project_terminal("p1", 24, 80)
        .expect("project terminal");
    let pid = engine.companion_terminals[&terminal_id]
        .client
        .process_session()
        .expect("a session")
        .sid;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while cwd_of(pid).as_deref() != Some(worktree.canonicalize().unwrap().as_path()) {
        assert!(std::time::Instant::now() < deadline, "the shell never cd'd");
        std::thread::sleep(Duration::from_millis(20));
    }

    let outcome =
        crate::engine::test_support::delete_through_pipeline(&mut engine, "s-agent", true, None);

    let removed = !worktree.exists();
    let still_there = cwd_of(pid);
    if let Some(terminal) = engine.companion_terminals.remove(&terminal_id) {
        terminal.client.force_terminate();
    }
    assert!(
        !(removed && still_there.is_some()),
        "the worktree at {} was removed while the project terminal dux started (pid {pid}) \
         was still working in it (cwd now {:?}; delete outcome ok: {})",
        worktree.display(),
        still_there,
        outcome.is_ok()
    );
}

fn alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    !stat
        .rsplit(')')
        .next()
        .is_some_and(|rest| rest.trim_start().starts_with('Z'))
}

/// The registry keeps only the newest `SESSIONS_PER_AGENT` (256) sessions per
/// agent and drops the oldest, whether or not anything still runs in it; its
/// comment says "the purge still follows every live process's own session",
/// but the purge only follows the sessions listed. An agent whose first
/// terminal left a server running, and which has since started 256 more
/// sessions (relaunches, terminals) in the same run, is deleted with its
/// worktree: the server is never ended and git removes the folder under it.
#[test]
fn an_old_session_with_a_live_job_is_not_forgotten_by_the_per_agent_cap() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    let pidfile = tmp.path().join("job.pid");
    // The agent's first terminal: starts a server and is closed.
    let client = crate::pty::PtyClient::spawn_with_env(
        "sh",
        &[
            "-c".to_string(),
            format!(
                "set -m; nohup sleep 300 >/dev/null 2>&1 & echo $! > '{}'",
                pidfile.display()
            ),
        ],
        &worktree,
        24,
        80,
        100,
        &[],
    )
    .expect("spawn sh");
    let process = client.process_session().expect("a session");
    engine
        .process_registry
        .register("s-agent", process, client.spawn_dir());
    client.set_leader_exit_hook(engine.process_registry.leader_exit_hook(process));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let job: i32 = loop {
        if let Ok(text) = std::fs::read_to_string(&pidfile)
            && let Ok(pid) = text.trim().parse()
        {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the job never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut client = client;
    while client.try_wait().is_none() {
        assert!(std::time::Instant::now() < deadline, "sh never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(client);
    engine
        .process_registry
        .wait_for_all_recordings(Duration::from_secs(5));
    // 256 later sessions of the same agent (tab relaunches, terminals), all
    // since ended.
    for n in 0..256u32 {
        engine.process_registry.register(
            "s-agent",
            crate::process_sessions::ProcessSession {
                sid: 3_000_000 + n,
                started_at: process.started_at,
                boot: process.boot,
            },
            &worktree,
        );
    }

    let outcome =
        crate::engine::test_support::delete_through_pipeline(&mut engine, "s-agent", true, None);
    let running = alive(job);
    let removed = !worktree.exists();
    if let Some(pid) = rustix::process::Pid::from_raw(job) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    assert!(
        !(running && removed),
        "the worktree at {} was removed while pid {job}, a server the agent's first terminal \
         left running there, was still running in it (delete outcome ok: {})",
        worktree.display(),
        outcome.is_ok()
    );
}

// Tests added with the fixes for the sixth review.

/// The refusal names the terminal standing in the worktree and says how to
/// get it out of the way; nothing is ended to make way for the removal.
#[test]
fn the_kept_message_names_the_terminal_standing_in_the_worktree() {
    let (mut engine, tmp) = test_engine();
    engine.config.shutdown_timeout_seconds = 1;
    let repo = repo(tmp.path());
    engine
        .projects
        .push(sample_project("p1", repo.to_str().unwrap()));
    let worktree = agent_worktree(&mut engine, tmp.path(), &repo, "agent");
    engine.config.terminal.command = "sh".to_string();
    engine.config.terminal.args = vec![
        "-c".to_string(),
        format!("cd '{}' && exec sleep 60", worktree.display()),
    ];
    let (terminal_id, _) = engine
        .create_project_terminal("p1", 24, 80)
        .expect("project terminal");
    let pid = engine.companion_terminals[&terminal_id]
        .client
        .process_session()
        .expect("a session")
        .sid;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while cwd_of(pid).as_deref() != Some(worktree.canonicalize().unwrap().as_path()) {
        assert!(std::time::Instant::now() < deadline, "the shell never cd'd");
        std::thread::sleep(Duration::from_millis(20));
    }
    let outcome =
        crate::engine::test_support::delete_through_pipeline(&mut engine, "s-agent", true, None);
    let alive = cwd_of(pid).is_some();
    if let Some(terminal) = engine.companion_terminals.remove(&terminal_id) {
        terminal.client.force_terminate();
    }
    let message = match &outcome {
        Err(message) => message.to_string(),
        Ok(_) => "the delete reported success".to_string(),
    };
    assert!(worktree.exists(), "kept");
    assert!(alive, "the terminal was not ended to make way");
    assert!(message.contains("project terminal"), "{message}");
    assert!(message.contains("cd"), "{message}");
}

/// A folder that is, or holds, a dux project's repository is never deleted
/// or moved by a destructive file operation, and the refusal names the
/// project.
#[test]
fn a_folder_holding_a_projects_repository_is_refused_naming_the_project() {
    let (mut engine, tmp) = test_engine();
    let outer = tmp.path().join("code");
    let repo = outer.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut project = sample_project("p1", repo.to_str().unwrap());
    project.name = "Shop".to_string();
    engine.projects.push(project);
    for target in [&outer, &repo] {
        let refused = engine
            .destructive_check(target)
            .clear("delete")
            .expect_err("a project's repository lives there");
        assert!(refused.0.contains("project \"Shop\""), "{refused}");
    }
    assert!(
        engine
            .destructive_check(&tmp.path().join("elsewhere"))
            .clear("delete")
            .is_ok()
    );
}
