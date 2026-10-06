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
    let out = Command::new("git")
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
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(
        path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
}

/// GET `path` from the dux serving on `port`, as a browser on this machine
/// would, and read its JSON.
fn get(port: u16, path: &str) -> serde_json::Value {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    let (head, body) = reply.split_once("\r\n\r\n").expect("a reply");
    assert!(head.starts_with("HTTP/1.1 200"), "{path}: {head}");
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{path}: {e}: {body}"))
}

/// Open a browser's terminal socket on `tab` of `agent`, the way the web UI
/// attaches to it, and keep it open for as long as the stream lives.
fn watch_tab(port: u16, agent: &str, tab: &str) -> std::net::TcpStream {
    use std::io::{BufRead, BufReader, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET /ws/sessions/{agent}/tabs/{tab}/pty HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
         Origin: http://127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .unwrap();
    let mut status = String::new();
    BufReader::new(stream.try_clone().unwrap())
        .read_line(&mut status)
        .unwrap();
    assert!(status.starts_with("HTTP/1.1 101"), "{status}");
    // The socket registers its attachment after its handshake: wait until
    // the agent counts it as a remote viewer.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let agents = get(port, "/api/v1/sessions");
        let viewers = agents
            .as_array()
            .unwrap()
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

    // The pull request's origin, served by the stand-in ssh.
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    // SAFETY: this binary runs this one test, and no other thread is running
    // yet.
    unsafe {
        // The config folder is home, so dux shortens its worktrees to `~/`
        // wherever it shows a label.
        std::env::set_var("HOME", &root);
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        for (name, value) in [
            ("GIT_AUTHOR_NAME", "dux"),
            ("GIT_AUTHOR_EMAIL", "dux@example.com"),
            ("GIT_COMMITTER_NAME", "dux"),
            ("GIT_COMMITTER_EMAIL", "dux@example.com"),
        ] {
            std::env::set_var(name, value);
        }
        std::env::set_var("GIT_SSH_COMMAND", bin.join("ssh"));
        std::env::set_var("GIT_SSH_VARIANT", "simple");
        let path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{path}", bin.display()));
    }
    git(&origin, &["init", "-q", "-b", "main"]);
    git(&origin, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&origin, &["update-ref", "refs/pull/42/head", "HEAD"]);
    executable(
        &bin.join("ssh"),
        &format!("#!/bin/sh\nexec git upload-pack '{}'\n", origin.display()),
    );
    executable(
        &bin.join("gh"),
        "#!/bin/sh\ncase \"$*\" in\n  \
         'auth status --active --json hosts') printf '%s' \
         '{\"hosts\":{\"github.com\":[{\"state\":\"success\",\"active\":true,\"host\":\"github.com\"}]}}' ;;\n  \
         'pr view 42 '*) printf '%s' \
         '{\"number\":42,\"title\":\"Fix it\",\"state\":\"OPEN\",\"headRefName\":\"feature/pr-42\"}' ;;\n  \
         *) exit 1 ;;\nesac\n",
    );

    let repo = root.join("hello");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:octo/hello.git"],
    );

    std::fs::write(
        root.join("config.toml"),
        "[providers.claude]\ncommand = \"cat\"\nargs = []\n\n[server]\ntailscale = \"no\"\n",
    )
    .unwrap();
    let port = serve(&root);

    // A project, added from its path.
    let run = ok(dux(
        &root,
        &["projects", "add", repo.to_str().unwrap(), "--yes"],
        "",
    ));
    let project = last_line(&run);
    let run = ok(dux(&root, &["projects", "ls", "-q"], ""));
    assert_eq!(run.stdout(), format!("{project}\n"));

    // An agent in it, waited for, its id printed last.
    let run = ok(dux(
        &root,
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
    let run = ok(dux(&root, &["agents", "ls", "--format", "json"], ""));
    let agents: serde_json::Value = serde_json::from_str(&run.stdout()).unwrap();
    assert_eq!(agents[0]["id"], agent.as_str(), "{agents}");
    assert_eq!(agents[0]["name"], "feat-a");
    assert_eq!(agents[0]["provider"], "claude");
    // The table's WORKTREE column is the whole path, as the JSON's is.
    let path = agents[0]["worktree_path"].as_str().unwrap().to_string();
    let run = ok(dux(&root, &["agents", "ls"], ""));
    let row = run.stdout().lines().nth(1).unwrap_or_default().to_string();
    assert!(row.contains(&format!(" {path} ")), "{}", run.stdout());

    // Its only tab cannot be closed: the engine's own sentence, and a
    // non-zero exit.
    let run = dux(
        &root,
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
    let run = ok(dux(
        &root,
        &["agents", "tabs", "add", "feat-a", "--yes"],
        "",
    ));
    let tab = last_line(&run);
    let run = ok(dux(&root, &["agents", "tabs", "ls", "feat-a", "-q"], ""));
    assert!(
        run.stdout().ends_with(&format!("{tab}\n")),
        "{}",
        run.stdout()
    );
    // A browser watching the tab stops a plain stop, which names it and
    // changes nothing; saying to go ahead over it stops the tab.
    let watcher = watch_tab(port, &agent, &tab);
    let run = dux(
        &root,
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
        &root,
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
        &root,
        &["agents", "tabs", "rm", "feat-a", &tab, "--yes"],
        "",
    ));

    // A project terminal, listed and closed.
    let run = ok(dux(
        &root,
        &["terminals", "add", "--project", "hello", "--yes"],
        "",
    ));
    let terminal = last_line(&run);
    let run = ok(dux(&root, &["terminals", "ls", "-q"], ""));
    assert_eq!(run.stdout(), format!("{terminal}\n"));
    ok(dux(&root, &["terminals", "rm", &terminal, "--yes"], ""));

    // The worktrees are the worktree manager's own entries.
    let run = ok(dux(
        &root,
        &["projects", "worktrees", "ls", "hello", "--format", "json"],
        "",
    ));
    let listed: serde_json::Value = serde_json::from_str(&run.stdout()).unwrap();
    let manager = get(port, &format!("/api/v1/projects/{project}/worktrees"));
    assert_eq!(listed, manager["entries"]);
    assert_eq!(listed[0]["agent_id"], agent.as_str(), "{listed}");

    // Deleting the agent with its worktree and keeping its branch says what
    // became of each.
    let worktree = listed[0]["worktree_path"].as_str().unwrap().to_string();
    let run = ok(dux(
        &root,
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
    assert_eq!(git(&repo, &["branch", "--list", "feat-a"]), "feat-a");
    let run = ok(dux(&root, &["agents", "ls", "-q"], ""));
    assert_eq!(run.stdout(), "");

    // An agent from a pull request, once dux has found gh.
    let deadline = Instant::now() + Duration::from_secs(30);
    while get(port, "/api/v1/bootstrap")["gh_available"] != true {
        assert!(Instant::now() < deadline, "dux never found the stand-in gh");
        std::thread::sleep(Duration::from_millis(100));
    }
    let run = ok(dux(
        &root,
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
    let run = ok(dux(&root, &["agents", "show", &from_pr], ""));
    assert!(
        run.stdout().contains("branch: feature/pr-42"),
        "{}",
        run.stdout()
    );

    let _ = std::fs::remove_dir_all(&root);
}
