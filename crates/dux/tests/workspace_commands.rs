//! `dux projects`, `agents`, `agents tabs`, `projects worktrees` and
//! `terminals` against a real dux: one served in this process on a temporary
//! config folder, with a git repository for a project, reached by the built
//! binary over the control socket.
//!
//! Nothing it runs is the developer's own: the provider is `cat`, `gh` is a
//! stand-in that knows one pull request, and the repository's GitHub remote
//! is reached through a stand-in ssh that serves a local repository.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use support::{Run, dux, serve};

fn home(name: &str) -> PathBuf {
    support::home("wscmd", name)
}

fn git(dir: &Path, args: &[&str]) -> String {
    // A fixture commit must not start background maintenance: since git 2.54
    // it prunes worktrees and can delete one dux is creating right after.
    let out = Command::new("git")
        .args(["-c", "maintenance.auto=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).expect("write the stand-in");
    std::fs::set_permissions(
        path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .expect("make the stand-in executable");
}

/// GET `path` from the dux serving on `port`, as a browser on this machine
/// would, and read its JSON.
fn get(port: u16, path: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("reach the dux over HTTP");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("send the request");
    let mut reply = String::new();
    stream.read_to_string(&mut reply).expect("read the reply");
    let (head, body) = reply.split_once("\r\n\r\n").expect("a reply");
    assert!(head.starts_with("HTTP/1.1 200"), "{path}: {head}");
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{path}: {e}: {body}"))
}

/// Open a browser's terminal socket on `tab` of `agent`, the way the web UI
/// attaches to it, and keep it open for as long as the stream lives.
fn watch_tab(port: u16, agent: &str, tab: &str) -> std::net::TcpStream {
    use std::io::{BufRead, BufReader, Write};
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("reach the dux over HTTP");
    write!(
        stream,
        "GET /ws/sessions/{agent}/tabs/{tab}/pty HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Origin: http://127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .expect("send the upgrade");
    let mut status = String::new();
    BufReader::new(stream.try_clone().expect("a second handle on the socket"))
        .read_line(&mut status)
        .expect("read the upgrade's answer");
    assert!(status.starts_with("HTTP/1.1 101"), "{status}");
    // The socket registers its attachment after its handshake: wait until
    // the agent counts it as a remote viewer.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let agents = get(port, "/api/v1/sessions");
        let viewers = agents
            .as_array()
            .expect("the sessions are a list")
            .iter()
            .find(|a| a["id"] == agent)
            .map(|a| a["remote_viewers"].clone());
        if viewers == Some(serde_json::json!(1)) {
            return stream;
        }
        assert!(Instant::now() < deadline, "the watcher never attached");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The last line `run` printed: the id a create prints after its sentence.
fn last_line(run: &Run) -> String {
    run.stdout().lines().last().unwrap_or_default().to_string()
}

fn ok(run: Run) -> Run {
    assert_eq!(run.code(), 0, "{}{}", run.stdout(), run.stderr());
    run
}

#[test]
fn projects_agents_tabs_worktrees_and_terminals_through_a_running_dux() {
    let root = home("running");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    isolate_from_the_developer(&root, &bin);
    stand_ins(&root, &bin);
    let repo = repository(&root, "hello");

    std::fs::write(
        root.join("config.toml"),
        "[providers.claude]\ncommand = \"cat\"\nargs = []\n\n[server]\ntailscale = \"no\"\n",
    )
    .unwrap();
    let port = serve(&root);

    let (project, agent) = a_project_and_an_agent_in_it(&root, &repo);
    the_agents_tabs(&root, port, &agent);
    a_project_terminal(&root);
    the_worktrees_then_the_delete(&root, port, &repo, &project, &agent);
    agents_from_pull_requests(&root, port);
    two_projects_on_one_repository_are_refused(&root);

    let _ = std::fs::remove_dir_all(&root);
}

/// Point git, the home folder and every program the serve runs at this test's
/// own: its git identity, its stand-in ssh first on the path.
fn isolate_from_the_developer(root: &Path, bin: &Path) {
    let path = std::env::var("PATH").unwrap_or_default();
    for (name, value) in [
        // The config folder is home, so dux shortens its worktrees to `~/`
        // wherever it shows a label.
        ("HOME", root.as_os_str().to_owned()),
        ("GIT_CONFIG_GLOBAL", "/dev/null".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("GIT_AUTHOR_NAME", "dux".into()),
        ("GIT_AUTHOR_EMAIL", "dux@example.com".into()),
        ("GIT_COMMITTER_NAME", "dux".into()),
        ("GIT_COMMITTER_EMAIL", "dux@example.com".into()),
        ("GIT_SSH_COMMAND", bin.join("ssh").into_os_string()),
        ("GIT_SSH_VARIANT", "simple".into()),
        ("PATH", format!("{}:{path}", bin.display()).into()),
    ] {
        // SAFETY: this binary runs this one test, and no other thread is
        // running yet.
        unsafe { std::env::set_var(name, value) };
    }
}

/// The pull requests' origins, served by the stand-in ssh, and the stand-in
/// gh that knows them.
fn stand_ins(root: &Path, bin: &Path) {
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).expect("make the origin");
    git(&origin, &["init", "-q", "-b", "main"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
    // A second repository, which no project has, with its own pull request.
    let other = root.join("other-origin");
    std::fs::create_dir_all(&other).expect("make the other origin");
    git(&other, &["init", "-q", "-b", "main"]);
    git(&other, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&other, &["update-ref", "refs/pull/5/head", "HEAD"]);
    executable(
        &bin.join("ssh"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  *octo/other*) exec git upload-pack '{}' ;;\n  \
             *) exec git upload-pack '{}' ;;\nesac\n",
            other.display(),
            origin.display()
        ),
    );
    executable(
        &bin.join("gh"),
        "#!/bin/sh\ncase \"$*\" in\n  \
         'auth status --active --json hosts') printf '%s' \
         '{\"hosts\":{\"github.com\":[{\"state\":\"success\",\"active\":true,\"host\":\"github.com\",\"gitProtocol\":\"ssh\"}]}}' ;;\n  \
         'pr view 5 '*) printf '%s' \
         '{\"number\":5,\"title\":\"Other\",\"state\":\"OPEN\",\"headRefName\":\"feature/other-5\"}' ;;\n  \
         'repo view '*) printf '%s' '{\"defaultBranchRef\":{\"name\":\"main\"}}' ;;\n  \
         'pr view 42 '*) printf '%s' \
         '{\"number\":42,\"title\":\"Fix it\",\"state\":\"OPEN\",\"headRefName\":\"feature/pr-42\"}' ;;\n  \
         *) exit 1 ;;\nesac\n",
    );
}

/// A repository `name` in `root` with one commit, whose GitHub remote is
/// octo/hello.
fn repository(root: &Path, name: &str) -> PathBuf {
    let repo = root.join(name);
    std::fs::create_dir_all(&repo).expect("make the repository");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:octo/hello.git"],
    );
    repo
}

/// A project, added from its path, and an agent in it; their ids.
fn a_project_and_an_agent_in_it(root: &Path, repo: &Path) -> (String, String) {
    let run = ok(dux(
        root,
        &[
            "projects",
            "add",
            repo.to_str().expect("a UTF-8 path"),
            "--yes",
        ],
        "",
    ));
    let project = last_line(&run);
    let run = ok(dux(root, &["projects", "ls", "-q"], ""));
    assert_eq!(run.stdout(), format!("{project}\n"));

    // An agent in it, waited for, its id printed last.
    let run = ok(dux(
        root,
        &[
            "agents",
            "add",
            "--project",
            "hello",
            "--name",
            "feat-a",
            "--yes",
        ],
        "",
    ));
    let agent = last_line(&run);
    let run = ok(dux(root, &["agents", "ls", "--format", "json"], ""));
    let agents: serde_json::Value =
        serde_json::from_str(&run.stdout()).expect("agents ls --format json is JSON");
    assert_eq!(agents[0]["id"], agent.as_str(), "{agents}");
    assert_eq!(agents[0]["name"], "feat-a");
    assert_eq!(agents[0]["provider"], "claude");
    // The table's WORKTREE column is the whole path, as the JSON's is.
    let path = agents[0]["worktree_path"]
        .as_str()
        .expect("a worktree path")
        .to_string();
    let run = ok(dux(root, &["agents", "ls"], ""));
    let row = run.stdout().lines().nth(1).unwrap_or_default().to_string();
    assert!(row.contains(&format!(" {path} ")), "{}", run.stdout());
    // The last column counts browsers, and nobody has the agent open yet.
    let header = run.stdout().lines().next().unwrap_or_default().to_string();
    assert!(header.trim_end().ends_with("TABS   BROWSERS"), "{header}");
    assert!(row.trim_end().ends_with(" 0"), "{row}");
    (project, agent)
}

/// The agent's only tab cannot be closed; a second one is stopped, over a
/// browser watching it only when told to, and closed.
fn the_agents_tabs(root: &Path, port: u16, agent: &str) {
    // Its only tab cannot be closed: the engine's own sentence, and a
    // non-zero exit.
    let run = dux(
        root,
        &["agents", "tabs", "rm", "feat-a", "claude", "--yes"],
        "",
    );
    assert_eq!(run.code(), 1, "{}", run.stderr());
    assert_eq!(
        run.stderr(),
        "This is the agent's only tab, so closing it would leave the agent with no tab at \
         all. Detach the agent instead to stop everything it is running, or add another tab \
         first.\n"
    );

    // A second tab, stopped, then closed.
    let run = ok(dux(root, &["agents", "tabs", "add", "feat-a", "--yes"], ""));
    let tab = last_line(&run);
    let run = ok(dux(root, &["agents", "tabs", "ls", "feat-a", "-q"], ""));
    assert!(
        run.stdout().ends_with(&format!("{tab}\n")),
        "{}",
        run.stdout()
    );
    // A browser watching the tab stops a plain stop, which names it and
    // changes nothing; saying to go ahead over it stops the tab.
    let watcher = watch_tab(port, agent, &tab);
    let run = dux(
        root,
        &["agents", "tabs", "stop", "feat-a", &tab, "--yes"],
        "",
    );
    assert_eq!(run.code(), 3, "{}", run.stderr());
    assert!(
        run.stderr().contains(&format!("tab {tab}")),
        "{}",
        run.stderr()
    );
    assert!(
        run.stderr().contains("--dangerously-ignore-connected"),
        "{}",
        run.stderr()
    );
    let run = ok(dux(
        root,
        &[
            "agents",
            "tabs",
            "stop",
            "feat-a",
            &tab,
            "--yes",
            "--dangerously-ignore-connected",
        ],
        "",
    ));
    assert!(run.stdout().starts_with("Stopped the "), "{}", run.stdout());
    drop(watcher);
    ok(dux(
        root,
        &["agents", "tabs", "rm", "feat-a", &tab, "--yes"],
        "",
    ));
}

/// A project terminal, listed and closed.
fn a_project_terminal(root: &Path) {
    let run = ok(dux(
        root,
        &["terminals", "add", "--project", "hello", "--yes"],
        "",
    ));
    let terminal = last_line(&run);
    let run = ok(dux(root, &["terminals", "ls", "-q"], ""));
    assert_eq!(run.stdout(), format!("{terminal}\n"));
    ok(dux(root, &["terminals", "rm", &terminal, "--yes"], ""));
}

/// The worktrees are the worktree manager's own entries, and deleting the
/// agent with its worktree and keeping its branch says what became of each.
fn the_worktrees_then_the_delete(root: &Path, port: u16, repo: &Path, project: &str, agent: &str) {
    let run = ok(dux(
        root,
        &["projects", "worktrees", "ls", "hello", "--format", "json"],
        "",
    ));
    let listed: serde_json::Value =
        serde_json::from_str(&run.stdout()).expect("worktrees ls --format json is JSON");
    let manager = get(port, &format!("/api/v1/projects/{project}/worktrees"));
    assert_eq!(listed, manager["entries"]);
    assert_eq!(listed[0]["agent_id"], agent, "{listed}");

    let worktree = listed[0]["worktree_path"]
        .as_str()
        .expect("a worktree path")
        .to_string();
    let run = ok(dux(
        root,
        &[
            "agents",
            "rm",
            "feat-a",
            "--delete-worktree",
            "--keep-branch",
            "--yes",
        ],
        "",
    ));
    assert!(
        run.stdout()
            .contains(&format!("worktree: {worktree} removed")),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout().contains("branch: feat-a kept"),
        "{}",
        run.stdout()
    );
    assert!(
        run.stdout()
            .contains("Its branch \"feat-a\" was kept because you chose to keep it."),
        "{}",
        run.stdout()
    );
    assert!(!Path::new(&worktree).exists());
    assert_eq!(git(repo, &["branch", "--list", "feat-a"]), "feat-a");
    let run = ok(dux(root, &["agents", "ls", "-q"], ""));
    assert_eq!(run.stdout(), "");
}

/// An agent from a pull request of the project's repository, and from one of
/// a repository no project has, which is cloned only when agreed to.
fn agents_from_pull_requests(root: &Path, port: u16) {
    // Once dux has found gh.
    let deadline = Instant::now() + Duration::from_secs(30);
    while get(port, "/api/v1/bootstrap")["gh_available"] != true {
        assert!(Instant::now() < deadline, "dux never found the stand-in gh");
        std::thread::sleep(Duration::from_millis(100));
    }
    let run = ok(dux(
        root,
        &[
            "agents",
            "add",
            "--from-pr",
            "https://github.com/octo/hello/pull/42",
            "--project",
            "hello",
            "--yes",
        ],
        "",
    ));
    let from_pr = last_line(&run);
    let run = ok(dux(root, &["agents", "show", &from_pr], ""));
    assert!(
        run.stdout().contains("branch: feature/pr-42"),
        "{}",
        run.stdout()
    );

    // A pull request of a repository no project has: refused before anything
    // is sent when it is a bare number, and without a terminal or --yes
    // nothing is cloned.
    let other_pr = "https://github.com/octo/other/pull/5";
    let run = dux(root, &["agents", "add", "--from-pr", "42", "--yes"], "");
    assert_ne!(run.code(), 0);
    assert!(run.stderr().contains("--project"), "{}", run.stderr());
    let run = dux(root, &["agents", "add", "--from-pr", other_pr], "");
    assert_ne!(run.code(), 0);
    assert!(
        run.stderr().contains("There is no terminal to ask on"),
        "{}",
        run.stderr()
    );
    let start = get(port, "/api/v1/browse")["path"]
        .as_str()
        .expect("the start folder")
        .to_string();
    let cloned = Path::new(&start).join("other");
    assert!(!cloned.exists());

    // With --yes it is cloned into the start folder, added as a project, and
    // the agent is on the pull request's branch.
    let run = ok(dux(
        root,
        &["agents", "add", "--from-pr", other_pr, "--yes"],
        "",
    ));
    let from_other = last_line(&run);
    assert!(cloned.join(".git").exists());
    let run = ok(dux(root, &["agents", "show", &from_other], ""));
    assert!(
        run.stdout().contains("branch: feature/other-5"),
        "{}",
        run.stdout()
    );

    // The repository now has a project, so --path has nothing to do.
    let run = dux(
        root,
        &[
            "agents",
            "add",
            "--from-pr",
            other_pr,
            "--path",
            "/nowhere",
            "--yes",
        ],
        "",
    );
    assert_ne!(run.code(), 0);
    assert!(
        run.stderr().contains("nothing to clone or add"),
        "{}",
        run.stderr()
    );
}

/// Two projects on one repository are refused, naming both.
fn two_projects_on_one_repository_are_refused(root: &Path) {
    let twin = repository(root, "hello-twin");
    ok(dux(
        root,
        &[
            "projects",
            "add",
            twin.to_str().expect("a UTF-8 path"),
            "--yes",
        ],
        "",
    ));
    let run = dux(
        root,
        &[
            "agents",
            "add",
            "--from-pr",
            "https://github.com/octo/hello/pull/42",
            "--yes",
        ],
        "",
    );
    assert_ne!(run.code(), 0);
    assert!(
        run.stderr().contains("hello") && run.stderr().contains("hello-twin"),
        "{}",
        run.stderr()
    );
}
