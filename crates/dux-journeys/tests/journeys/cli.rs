//! The command line against a real dux: `dux projects`, `agents`, `tabs`,
//! `worktrees`, `terminals` and `operations` over the control socket, a saved
//! remote reached over HTTP with and without a password, `dux server logs` and
//! `connections`, and `dux config set` saying how the running dux took the
//! change. Every command runs inside the dux container, as a person at that
//! machine types it.

use std::time::Duration;

use dux_journeys::dux::Exec;
use dux_journeys::util::shell_quote;
use dux_journeys::ws::connect_ok;
use dux_journeys::{Dux, DuxOptions, OTHER_STRONG_PASSWORD, STRONG_PASSWORD, eventually, journey};

/// A config folder of its own for the command line, so the remote it talks
/// to is another dux, not this machine's.
const CLI_HOME: &str = "/tmp/cli-home";

/// A browser's `User-Agent`, so the connection is listed as a device.
const CHROME_ON_LINUX: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// `dux <args>` inside the container, against its running dux.
async fn cli(dux: &Dux, args: &str) -> Exec {
    dux.exec(&format!("dux {args}")).await
}

/// [`cli`], panicking with what it printed unless it exited 0.
async fn cli_ok(dux: &Dux, args: &str) -> Exec {
    let run = cli(dux, args).await;
    assert_eq!(run.code, 0, "dux {args}:\n{}", run.output());
    run
}

/// `dux <args>` with the command line's own config folder.
async fn remote_cli(dux: &Dux, args: &str) -> Exec {
    dux.exec(&format!("DUX_HOME={CLI_HOME} dux {args}")).await
}

/// `dux remote login <name> --stdin`, the password piped in.
async fn remote_login(dux: &Dux, name: &str, password: &str) -> Exec {
    dux.exec(&format!(
        "printf %s {} | DUX_HOME={CLI_HOME} dux remote login {name} --stdin",
        shell_quote(password)
    ))
    .await
}

/// The last line a command printed: the id a create prints after its sentence.
fn last_line(run: &Exec) -> String {
    run.stdout.lines().last().unwrap_or_default().to_string()
}

/// The agent `id` in `dux agents ls --format json`.
async fn listed_agent(dux: &Dux, id: &str) -> Option<serde_json::Value> {
    let run = cli_ok(dux, "agents ls --format json").await;
    let agents: serde_json::Value = serde_json::from_str(&run.stdout)
        .unwrap_or_else(|err| panic!("agents ls --format json is JSON: {err}: {}", run.stdout));
    agents
        .as_array()?
        .iter()
        .find(|a| a["id"].as_str() == Some(id))
        .cloned()
}

/// The state `dux agents tabs ls` shows for `tab` of `agent`.
async fn tab_state(dux: &Dux, agent: &str, tab: &str) -> Option<String> {
    let run = cli_ok(dux, &format!("agents tabs ls {agent} --format json")).await;
    let tabs: serde_json::Value = serde_json::from_str(&run.stdout).ok()?;
    tabs.as_array()?
        .iter()
        .find(|t| t["id"].as_str() == Some(tab))
        .and_then(|t| t["state"].as_str())
        .map(str::to_string)
}

/// Situation: a fresh `dux server` reachable from the network, no password,
/// the demo repository on disk, and nothing added yet.
///
/// Task: the owner drives every resource dux keeps in its database from a
/// shell on that machine: projects, agents, their tabs and worktrees, and
/// terminals, without a browser. A delete must not cut off somebody watching
/// from a browser unless they say so in so many words, and a change must not
/// answer before it has really finished.
///
/// Action: point new agents at the fake provider with `dux config set`; add the
/// demo repository as a project, list and show it; create an agent and list
/// and show it; add a tab, start, stop and close it; stop and start the agent;
/// open an agent, a project and a standalone terminal, list them and close
/// them; list the project's worktrees. Then open the agent's terminal from a
/// browser, delete the agent with its worktree, and delete it again with
/// `--dangerously-ignore-connected`. Create a second agent and delete it with
/// `--no-wait`, then look it up with `dux operations show`. Remove the project.
///
/// Result: `config set` says the running dux reloaded; each create prints the
/// new id last and every list shows exactly what was made; the tab reads
/// running once started and stopped once stopped; every terminal is listed
/// with its owner and none remains once closed; the worktree list names the
/// agent as the holder. With the browser watching, the delete exits 3, names
/// the browser and the flag and changes nothing; with the flag it exits 0
/// saying the worktree was removed and the branch kept, and the worktree is
/// already gone from disk when the command returns while the branch is still
/// in the repository. The `--no-wait` delete prints an operation id that
/// `operations show` reports succeeded, and nothing is listed once the project
/// is removed.
#[tokio::test(flavor = "multi_thread")]
async fn cli_projects_agents_tabs_and_terminals_over_the_socket() {
    journey("cli-resources", Duration::from_secs(360), async {
        let dux = Dux::start(DuxOptions::exposed()).await;

        let set = cli_ok(&dux, "config set defaults.provider fake").await;
        assert!(
            set.stdout
                .contains("Configuration reloaded. New settings are active now."),
            "config set says the running dux reloaded: {}",
            set.output()
        );

        let added = cli_ok(&dux, "projects add /repos/demo-api --yes").await;
        let project = last_line(&added);
        assert_eq!(
            cli_ok(&dux, "projects ls -q").await.stdout,
            format!("{project}\n")
        );
        let table = cli_ok(&dux, "projects ls").await.stdout;
        assert!(
            table.starts_with("ID") && table.contains("demo-api"),
            "projects ls is a table naming the project: {table}"
        );
        let shown = cli_ok(&dux, "projects show demo-api").await.stdout;
        assert!(shown.contains("/repos/demo-api"), "{shown}");

        let created = cli_ok(&dux, "agents add --project demo-api --name cli-a --yes").await;
        let agent = last_line(&created);
        let listed = listed_agent(&dux, &agent)
            .await
            .unwrap_or_else(|| panic!("the new agent {agent} is listed"));
        assert_eq!(listed["name"], "cli-a", "{listed}");
        assert_eq!(listed["provider"], "fake", "{listed}");
        let worktree = listed["worktree_path"]
            .as_str()
            .expect("a managed agent has a worktree")
            .to_string();
        let shown = cli_ok(&dux, "agents show cli-a").await.stdout;
        assert!(shown.contains("branch: cli-a"), "{shown}");
        let first_tab = cli_ok(&dux, "agents tabs ls cli-a -q")
            .await
            .stdout
            .lines()
            .next()
            .expect("an agent has a first tab")
            .to_string();

        let tab = last_line(&cli_ok(&dux, "agents tabs add cli-a --yes").await);
        let tabs = cli_ok(&dux, "agents tabs ls cli-a -q").await.stdout;
        assert_eq!(tabs, format!("{first_tab}\n{tab}\n"));
        cli_ok(&dux, &format!("agents tabs start cli-a {tab} --yes")).await;
        eventually("the new tab to run", Duration::from_secs(30), || async {
            tab_state(&dux, &agent, &tab)
                .await
                .filter(|state| state != "stopped")
        })
        .await;
        cli_ok(&dux, &format!("agents tabs stop cli-a {tab} --yes")).await;
        assert_eq!(
            tab_state(&dux, &agent, &tab).await.as_deref(),
            Some("stopped"),
            "a waited stop has stopped the tab when it returns"
        );
        cli_ok(&dux, &format!("agents tabs rm cli-a {tab} --yes")).await;
        assert_eq!(
            cli_ok(&dux, "agents tabs ls cli-a -q").await.stdout,
            format!("{first_tab}\n")
        );

        cli_ok(&dux, "agents stop cli-a --yes").await;
        cli_ok(&dux, "agents start cli-a --yes").await;

        let mut terminals = Vec::new();
        for owner in ["--agent cli-a", "--project demo-api", ""] {
            let opened = cli_ok(&dux, &format!("terminals add {owner} --yes")).await;
            terminals.push(last_line(&opened));
        }
        let table = cli_ok(&dux, "terminals ls").await.stdout;
        for owner in ["agent cli-a", "project demo-api", "standalone in"] {
            assert!(table.contains(owner), "terminals ls names {owner}: {table}");
        }
        let mut ids: Vec<String> = cli_ok(&dux, "terminals ls -q")
            .await
            .stdout
            .lines()
            .map(str::to_string)
            .collect();
        ids.sort();
        let mut expected = terminals.clone();
        expected.sort();
        assert_eq!(ids, expected);
        for terminal in &terminals {
            cli_ok(&dux, &format!("terminals rm {terminal} --yes")).await;
        }
        assert_eq!(cli_ok(&dux, "terminals ls -q").await.stdout, "");

        let run = cli_ok(&dux, "projects worktrees ls demo-api --format json").await;
        let entries: serde_json::Value = serde_json::from_str(&run.stdout).expect("JSON");
        let held = entries
            .as_array()
            .expect("a list")
            .iter()
            .find(|e| e["worktree_path"].as_str() == Some(worktree.as_str()))
            .unwrap_or_else(|| panic!("the agent's worktree is listed: {entries}"));
        assert_eq!(held["agent_id"], agent.as_str(), "{held}");

        // A browser on another machine watches the agent's terminal.
        let browser = dux.client().await;
        let mut watcher = connect_ok(
            &browser,
            &format!("/ws/sessions/{agent}/tabs/{first_tab}/pty"),
        )
        .await;
        eventually(
            "the agent to count the browser",
            Duration::from_secs(20),
            || async {
                let listed = listed_agent(&dux, &agent).await?;
                (listed["remote_viewers"] == 1).then_some(())
            },
        )
        .await;

        let refused = cli(
            &dux,
            "agents rm cli-a --delete-worktree --keep-branch --yes",
        )
        .await;
        assert_eq!(refused.code, 3, "{}", refused.output());
        assert!(
            refused
                .stderr
                .contains("Someone else is using this right now")
                && refused
                    .stderr
                    .contains(&format!("watching tab {first_tab}"))
                && refused.stderr.contains("--dangerously-ignore-connected"),
            "the refusal names the watcher and the way past it: {}",
            refused.stderr
        );
        assert!(
            listed_agent(&dux, &agent).await.is_some(),
            "a refused delete changes nothing"
        );
        dux.exec_ok(&format!("test -d {}", shell_quote(&worktree)))
            .await;

        let deleted = cli_ok(
            &dux,
            "agents rm cli-a --delete-worktree --keep-branch --yes \
             --dangerously-ignore-connected",
        )
        .await;
        // Checked at once, with no polling: the command answers only once the
        // removal has really happened.
        let gone = dux
            .exec(&format!("test -e {}", shell_quote(&worktree)))
            .await;
        assert_ne!(gone.code, 0, "the worktree is gone when the delete returns");
        assert!(
            deleted
                .stdout
                .contains(&format!("worktree: {worktree} removed"))
                && deleted.stdout.contains("branch: cli-a kept"),
            "the delete says what became of each piece: {}",
            deleted.stdout
        );
        assert_eq!(
            dux.exec_ok("git -C /repos/demo-api branch --list cli-a")
                .await
                .trim(),
            "cli-a"
        );
        assert!(
            watcher.wait_ended(Duration::from_secs(10)).await.is_some(),
            "the browser's terminal ends with the agent"
        );

        let second =
            last_line(&cli_ok(&dux, "agents add --project demo-api --name cli-b --yes").await);
        let started = cli_ok(&dux, "agents rm cli-b --delete-worktree --yes --no-wait").await;
        let operation = started.stdout.trim().to_string();
        assert_eq!(
            operation.lines().count(),
            1,
            "--no-wait prints the operation id alone: {}",
            started.output()
        );
        let shown = eventually("the delete to finish", Duration::from_secs(60), || async {
            let run = cli(&dux, &format!("operations show {operation}")).await;
            (run.code != 6).then_some(run)
        })
        .await;
        assert_eq!(shown.code, 0, "{}", shown.output());
        assert!(
            shown.stdout.contains("state:    succeeded"),
            "{}",
            shown.stdout
        );
        assert!(listed_agent(&dux, &second).await.is_none());

        cli_ok(&dux, "projects rm demo-api --yes").await;
        assert_eq!(cli_ok(&dux, "projects ls -q").await.stdout, "");
    })
    .await;
}

/// Situation: a `dux server` on loopback with no password yet and
/// `[server.auth] require = "everywhere"`, and a command line on the same
/// machine with a config folder of its own, so the dux is a remote to it,
/// reached over HTTP.
///
/// Task: the owner saves the remote and uses it; once the remote has a
/// password, the command line signs in, and a password change ends that
/// sign-in.
///
/// Action: save the remote; list its projects; try to sign in; set the remote's
/// password with `dux config set` on its own machine; list again; sign in with
/// the password; list through `DUX_REMOTE`; change the password; list again;
/// sign in with the new one; sign out; list again.
///
/// Result: with no password the list answers with no sign-in and the login
/// says there is nothing to sign in to; setting the password reports the
/// running dux has it in force; then the list exits 5 naming
/// `dux remote login`; after signing in it answers, through the variable too;
/// after the password change the list exits 5 again, the old sign-in ended;
/// the new password signs in again; after signing out the list exits 5.
#[tokio::test(flavor = "multi_thread")]
async fn cli_a_remote_with_and_without_a_password() {
    journey("cli-remote", Duration::from_secs(240), async {
        let dux =
            Dux::start(DuxOptions::local().with_config("server.auth.require", "everywhere")).await;
        let needs_login = "box asks for its password; run \"dux remote login box\"";

        dux.exec_ok(&format!("mkdir -m 700 -p {CLI_HOME}")).await;
        let saved = remote_cli(&dux, "remote add box http://127.0.0.1:3890").await;
        assert_eq!(saved.code, 0, "{}", saved.output());

        let open = remote_cli(&dux, "--remote box projects ls -q").await;
        assert_eq!(open.code, 0, "no password, no sign-in: {}", open.output());
        let nothing = remote_login(&dux, "box", "anything-at-all").await;
        assert_eq!(nothing.code, 0, "{}", nothing.output());
        assert!(
            nothing.stdout.contains("box has no password"),
            "{}",
            nothing.output()
        );

        let set = dux
            .config_set_secret("server.auth.password", STRONG_PASSWORD)
            .await;
        assert_eq!(set.code, 0, "{}", set.output());
        assert!(
            set.stdout.contains("The new password is in force"),
            "config set says the running dux has the password: {}",
            set.output()
        );

        let refused = remote_cli(&dux, "--remote box projects ls").await;
        assert_eq!(refused.code, 5, "{}", refused.output());
        assert_eq!(refused.stderr.trim(), needs_login);

        let signed_in = remote_login(&dux, "box", STRONG_PASSWORD).await;
        assert_eq!(signed_in.code, 0, "{}", signed_in.output());
        assert_eq!(signed_in.stdout.trim(), "Signed in to box.");
        let listed = remote_cli(&dux, "--remote box projects ls -q").await;
        assert_eq!(listed.code, 0, "{}", listed.output());
        let by_variable = dux
            .exec(&format!(
                "DUX_REMOTE=box DUX_HOME={CLI_HOME} dux projects ls -q"
            ))
            .await;
        assert_eq!(by_variable.code, 0, "{}", by_variable.output());

        let changed = dux
            .config_set_secret("server.auth.password", OTHER_STRONG_PASSWORD)
            .await;
        assert_eq!(changed.code, 0, "{}", changed.output());
        let signed_out = remote_cli(&dux, "--remote box projects ls").await;
        assert_eq!(signed_out.code, 5, "{}", signed_out.output());
        assert_eq!(signed_out.stderr.trim(), needs_login);

        let again = remote_login(&dux, "box", OTHER_STRONG_PASSWORD).await;
        assert_eq!(again.code, 0, "{}", again.output());
        assert_eq!(
            remote_cli(&dux, "--remote box projects ls -q").await.code,
            0
        );
        let logout = remote_cli(&dux, "remote logout box").await;
        assert_eq!(logout.code, 0, "{}", logout.output());
        assert_eq!(logout.stdout.trim(), "Signed out of box.");
        assert_eq!(remote_cli(&dux, "--remote box projects ls").await.code, 5);
    })
    .await;
}

/// Situation: a `dux server` reachable from the network with its access log on
/// (the default), and a browser on another machine.
///
/// Task: the owner wants to see, from a shell, who is connected and what the
/// server is logging as it happens.
///
/// Action: open a standalone terminal; open the browser's terminal socket on it
/// and type into it; list the connections. Start `dux server logs -f`; ask the
/// server for a path nobody links to; read what the follow printed; print the
/// last lines with `dux server logs`.
///
/// Result: the connections list has the browser as "Chrome on Linux", driving
/// that terminal. The follow prints the request's line while it runs, and
/// `dux server logs` shows it too.
#[tokio::test(flavor = "multi_thread")]
async fn cli_server_logs_follow_and_connections() {
    journey("cli-server-inspect", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed()).await;

        let terminal = last_line(&cli_ok(&dux, "terminals add --yes").await);
        let browser = dux
            .client()
            .await
            .with_header("User-Agent", CHROME_ON_LINUX);
        let mut pty = connect_ok(&browser, &format!("/ws/terminals/{terminal}/pty")).await;
        pty.next_event("connected", Duration::from_secs(20))
            .await
            .expect("the terminal socket says it is connected");
        pty.claim(24, 80).await;
        let row = eventually(
            "the browser to be listed driving the terminal",
            Duration::from_secs(20),
            || async {
                let listed = cli_ok(&dux, "server connections ls").await.stdout;
                listed
                    .lines()
                    .find(|line| {
                        line.contains("Chrome on Linux")
                            && line.contains(&format!("terminal {terminal} (driving)"))
                    })
                    .map(str::to_string)
            },
        )
        .await;
        assert!(!row.is_empty());

        let follower = dux
            .exec_ok("setsid dux server logs -f > /tmp/follow.out 2>&1 < /dev/null & echo $!")
            .await
            .trim()
            .to_string();
        // The follow is running once it has printed what it starts from.
        eventually("the follow to start", Duration::from_secs(20), || async {
            let out = dux.exec_ok("cat /tmp/follow.out").await;
            (!out.is_empty()).then_some(())
        })
        .await;
        // A path nobody links to, so its line can only be this request's.
        let marker = format!("/journey-marker-{}", dux_journeys::util::suffix());
        browser.get(&marker).await;
        eventually(
            "the follow to print the request",
            Duration::from_secs(20),
            || async {
                let out = dux.exec_ok("cat /tmp/follow.out").await;
                out.contains(&marker).then_some(())
            },
        )
        .await;
        dux.exec_ok(&format!("kill {follower}")).await;

        let tail = cli_ok(&dux, "server logs --lines 1000").await;
        assert!(tail.stdout.contains(&marker), "{}", tail.output());
        pty.close().await;
    })
    .await;
}

/// Situation: a `dux server` running in its container, and the same server
/// stopped for a moment.
///
/// Task: the owner changes settings with `dux config set` and needs to know
/// what the running dux did with each change, not only that the file changed.
///
/// Action: set a setting the running dux applies at once; set the port, which
/// a running server reads only when it binds; stop dux and set a setting.
///
/// Result: the first says the running dux reloaded and the new settings are
/// active; the port says it reloaded and that the server must restart to use
/// the port; with dux stopped it says the change applies the next time dux
/// starts. Each exits 0.
#[tokio::test(flavor = "multi_thread")]
async fn cli_config_set_reports_how_the_running_dux_took_it() {
    journey("cli-config-set", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed().with_restart_loop()).await;

        let applied = dux.config_set("ui.left_width_pct", "30").await;
        assert_eq!(applied.code, 0, "{}", applied.output());
        assert!(
            applied
                .stdout
                .contains("Configuration reloaded. New settings are active now."),
            "{}",
            applied.output()
        );

        let port = dux.config_set("server.port", "3999").await;
        assert_eq!(port.code, 0, "{}", port.output());
        assert!(
            port.stdout
                .contains("Configuration reloaded. New settings are active now.")
                && port.stdout.contains("port")
                && port.stdout.contains("restart the server to apply them"),
            "{}",
            port.output()
        );

        let pid = dux.dux_pid().await.expect("dux is running");
        let stopped = dux
            .exec(&format!(
                "touch /tmp/dux-hold && kill -TERM {pid} && \
                 while pgrep -x dux > /dev/null; do sleep 0.2; done && \
                 dux config set ui.left_width_pct 35; code=$?; rm -f /tmp/dux-hold; exit $code"
            ))
            .await;
        assert_eq!(stopped.code, 0, "{}", stopped.output());
        assert!(
            stopped
                .stdout
                .contains("dux is not running, so the change applies the next time it starts."),
            "{}",
            stopped.output()
        );
    })
    .await;
}
