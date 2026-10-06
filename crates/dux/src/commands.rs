//! The one command tree of the `dux` binary. Everything the command line
//! accepts is declared here and parsed before a lock is taken or an engine
//! starts; `main` only dispatches on the result.

use clap::{Args, Parser, Subcommand, ValueEnum};

const SERVER_ABOUT: &str = "\
Run the dux web UI over the headless engine. The web UI has one optional
password for one owner, and everyone who gets in shares the one workspace. Set
it with `dux config set server.auth.password`; [server.auth] require decides
who is asked for it (by default everyone but this machine and your tailnet).
With no password, anyone who can reach a non-loopback address controls your
agents and terminals, and dux says so loudly as it starts.";

const ROOT_AFTER_HELP: &str = "\
Environment variables:
  DUX_HOME    Override the config directory (must be an absolute path).
              When unset, defaults to:
                macOS: ~/.dux/
                Linux: $XDG_CONFIG_HOME/dux/ or ~/.config/dux/
  DUX_REMOTE  The saved remote to talk to when neither --remote nor --local
              is given. It wins over the default set with `dux remote default`.

First run writes a full default config to config.toml in that directory, and
session state is stored beside it in sessions.sqlite3.

Projects, agents, terminals, `dux server connections` and `dux operations`
need a running dux: the one on this machine, or a saved remote. Macros,
providers, keys, themes and env read and edit config.toml when dux is stopped,
and `dux server logs` reads server.log. These commands end with these exit
codes:
  0  success
  1  the change failed, or what was named does not exist
  2  the command line is wrong
  3  refused: someone is attached, another change is in the way, or the
     change was not confirmed
  4  dux isn't running, or does not answer
  5  the remote asks for its password; sign in with `dux remote login`
  6  the outcome is unknown: the wait ran out, and the change keeps running";

#[derive(Parser, Debug)]
#[command(
    name = "dux",
    version,
    about = "Terminal and web UI for AI worktree sessions, and a command line for their state. Run with no command to launch the terminal UI.",
    after_help = ROOT_AFTER_HELP
)]
pub struct Cli {
    /// Talk to this saved remote instead of the local dux.
    #[arg(long, global = true, value_name = "NAME")]
    pub remote: Option<String>,

    /// Talk to the dux on this machine, whatever a saved default says.
    #[arg(long, global = true, conflicts_with = "remote")]
    pub local: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Serve the web UI over the headless engine, or inspect a running server.
    Server(ServerCmd),
    /// Manage the configuration file on this machine.
    #[command(disable_help_flag = true)]
    Config(ConfigCmd),
    /// Projects dux knows about.
    Projects(ProjectsCmd),
    /// Agents, their tabs and their worktrees.
    Agents(AgentsCmd),
    /// Terminals, project-owned, agent-owned and standalone.
    Terminals(TerminalsCmd),
    /// Macros from config.toml.
    Macros(MacrosCmd),
    /// Providers from config.toml.
    Providers(NamedReadCmd),
    /// Keybindings from config.toml.
    Keys(ListOnlyCmd),
    /// Themes dux can load.
    Themes(ListOnlyCmd),
    /// The global environment from config.toml.
    Env(EnvCmd),
    /// Saved remote duxes and the sign-ins for them.
    Remote(RemoteCmd),
    /// Look up a change that was started earlier.
    Operations(OperationsCmd),
}

// ---------------------------------------------------------------------------
// server and config
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
#[command(before_help = SERVER_ABOUT)]
pub struct ServerCmd {
    /// Bind this exact address, overriding [server] host and port. An IP:port
    /// socket address (hostnames are NOT resolved), e.g. 0.0.0.0:3890.
    #[arg(long, value_name = "ADDR:PORT")]
    pub bind: Option<String>,

    /// Override [server] port only (ignored when --bind is set). dux binds
    /// host:port, and the machine's Tailscale address unless disabled.
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,

    /// Skip Tailscale detection this run (serve the configured host only).
    #[arg(long)]
    pub no_tailscale: bool,

    #[command(subcommand)]
    pub command: Option<ServerSub>,
}

impl ServerCmd {
    pub fn into_overrides(self) -> dux_core::config::ServerCliOverrides {
        dux_core::config::ServerCliOverrides {
            bind: self.bind,
            port: self.port,
            no_tailscale: self.no_tailscale,
        }
    }
}

/// The listener flags only make sense when starting a server, so they are
/// refused beside `logs` and `connections`. The global flags are not.
pub fn listener_flags_with_subcommand(server: &ServerCmd) -> Result<(), String> {
    let set = server.bind.is_some() || server.port.is_some() || server.no_tailscale;
    if set && server.command.is_some() {
        return Err(
            "--bind, --port and --no-tailscale start a server and cannot be used with a server subcommand"
                .to_string(),
        );
    }
    Ok(())
}

/// Starting a server is always on this machine, so naming a target is a
/// mistake.
pub fn check_server_start_target(remote: Option<&str>, local: bool) -> Result<(), String> {
    if remote.is_some() || local {
        return Err("dux server starts a server on this machine; --remote and --local apply to its logs and connections subcommands".to_string());
    }
    Ok(())
}

/// What `dux [--remote x | --local] config <args>` carries: the global flags
/// that came before the word, and everything after it, byte for byte.
#[derive(Debug)]
pub struct ConfigInvocation {
    pub remote: Option<String>,
    pub local: bool,
    pub args: Vec<String>,
}

/// Splits off a command line whose command is `config`, after the global flags alone, so clap
/// never sees its words. Any other shape, or one naming both global flags, is left to clap.
pub fn split_config(args: &[String]) -> Option<ConfigInvocation> {
    let mut remote = None;
    let mut local = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "config" {
            if remote.is_some() && local {
                return None;
            }
            return Some(ConfigInvocation {
                remote,
                local,
                args: args[i + 1..].to_vec(),
            });
        }
        match arg {
            "--local" => local = true,
            "--remote" => {
                i += 1;
                remote = Some(args.get(i)?.clone());
            }
            _ => remote = Some(arg.strip_prefix("--remote=")?.to_string()),
        }
        i += 1;
    }
    None
}

/// `dux config` edits this machine's file, so a remote selected by `--remote`, `DUX_REMOTE` or
/// the saved default is refused unless `--local` is given.
pub fn check_config_target(remote: Option<&str>, local: bool) -> Result<(), String> {
    if remote.is_some() && !local {
        return Err(
            "dux config edits this machine's config.toml; run \"dux --local config …\" to go ahead, or unset DUX_REMOTE"
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Subcommand, Debug)]
pub enum ServerSub {
    /// Show the server's log: the last lines of server.log, read from the
    /// file when dux is not running.
    Logs {
        /// Keep printing new lines as they are written, until interrupted.
        #[arg(short = 'f', long)]
        follow: bool,
        /// How many of the last lines to start from.
        #[arg(long, value_name = "N", default_value_t = dux_core::client::server_inspect::DEFAULT_LINES)]
        lines: usize,
    },
    /// The connections attached to the server.
    Connections {
        #[command(subcommand)]
        command: ConnectionsSub,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConnectionsSub {
    /// List the connections.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
}

/// `dux config` hands its arguments, untouched, to the config code, which
/// owns every message and exit code of that command.
#[derive(Args, Debug)]
pub struct ConfigCmd {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARGS"
    )]
    pub args: Vec<String>,
}

// ---------------------------------------------------------------------------
// flags shared by resource commands
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Format {
    Table,
    Json,
}

#[derive(Args, Debug)]
pub struct ListFlags {
    /// Output format.
    #[arg(long, value_enum, default_value = "table")]
    pub format: Format,
    /// Print only the ids, one per line.
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

/// What every change takes: whether to confirm, and how long to wait.
#[derive(Args, Debug)]
pub struct ChangeFlags {
    /// Do not ask for confirmation (required without a terminal).
    #[arg(long)]
    pub yes: bool,
    /// Print the operation id and return without waiting for the change.
    #[arg(long)]
    pub no_wait: bool,
    /// Wait at most this many seconds for the change to finish.
    #[arg(long, value_name = "SECONDS", conflicts_with = "no_wait")]
    pub wait_timeout: Option<u64>,
}

/// A change that can cut off whoever is attached to what it touches.
#[derive(Args, Debug)]
pub struct GuardedChangeFlags {
    #[command(flatten)]
    pub change: ChangeFlags,
    /// Go ahead even though someone is attached.
    #[arg(long)]
    pub dangerously_ignore_connected: bool,
}

/// `dux agents add`: one of a project, `--fork` or `--standalone` says
/// where the agent comes from.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("source")
        .args(["project", "fork", "standalone"])
        .required(true)
))]
pub struct AddAgentArgs {
    /// The project to create the agent in, on a new branch unless
    /// --from-pr or --from-worktree says otherwise.
    #[arg(long, value_name = "PROJECT", conflicts_with_all = ["fork", "standalone"])]
    pub project: Option<String>,
    /// The agent's name, which is also its branch's name in a project.
    #[arg(long)]
    pub name: Option<String>,
    /// Create the agent on the branch of that name that already exists.
    #[arg(long, conflicts_with_all = ["from_pr", "from_worktree", "fork", "standalone"])]
    pub existing_branch: bool,
    /// Create the agent from this pull request of the project (a number or
    /// its address).
    #[arg(
        long,
        value_name = "PR",
        requires = "project",
        conflicts_with = "from_worktree"
    )]
    pub from_pr: Option<String>,
    /// Create the agent on this worktree the project already has.
    #[arg(long, value_name = "PATH", requires = "project")]
    pub from_worktree: Option<String>,
    /// Create a standalone agent in this folder of your own, with no project.
    #[arg(long, value_name = "FOLDER", conflicts_with = "fork")]
    pub standalone: Option<String>,
    /// The provider a standalone agent runs (the global default otherwise).
    #[arg(long, value_name = "NAME", conflicts_with_all = ["project", "fork"])]
    pub provider: Option<String>,
    /// Fork this agent: a new worktree from its branch.
    #[arg(long, value_name = "AGENT")]
    pub fork: Option<String>,
    /// Copy the uncommitted changes of the project's checkout into the new
    /// worktree.
    #[arg(long, conflicts_with_all = ["from_pr", "from_worktree", "fork", "standalone"])]
    pub copy_uncommitted: bool,
    #[command(flatten)]
    pub change: ChangeFlags,
}

/// `dux terminals add`: an agent's or a project's, or a standalone one when
/// neither is named.
#[derive(Args, Debug)]
pub struct AddTerminalArgs {
    /// Open it in this agent's worktree.
    #[arg(long, value_name = "AGENT", conflicts_with = "project")]
    pub agent: Option<String>,
    /// Open it at this project's root.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,
    #[command(flatten)]
    pub change: ChangeFlags,
}

// ---------------------------------------------------------------------------
// resources
// ---------------------------------------------------------------------------

#[derive(Args, Debug)]
pub struct ProjectsCmd {
    #[command(subcommand)]
    pub command: ProjectsSub,
}

#[derive(Subcommand, Debug)]
pub enum ProjectsSub {
    /// List projects.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one project.
    Show { project: String },
    /// Add a project from a path and print its id last.
    Add {
        path: String,
        /// The project's name (the folder's name by default).
        #[arg(long)]
        name: Option<String>,
        /// Check the repository's default branch out first.
        #[arg(long)]
        checkout_default: bool,
        /// Make a plain folder a git repository first, with a first commit.
        #[arg(long)]
        init: bool,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Remove a project. Its agents' worktrees stay on disk unless
    /// --delete-worktrees is given.
    #[command(visible_alias = "remove")]
    Rm {
        project: String,
        /// Also delete its agents and their worktrees.
        #[arg(long)]
        delete_worktrees: bool,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// The worktrees of a project.
    Worktrees {
        #[command(subcommand)]
        command: WorktreesSub,
    },
}

#[derive(Subcommand, Debug)]
pub enum WorktreesSub {
    /// List a project's worktrees.
    #[command(visible_alias = "list")]
    Ls {
        project: String,
        #[command(flatten)]
        list: ListFlags,
    },
}

#[derive(Args, Debug)]
pub struct AgentsCmd {
    #[command(subcommand)]
    pub command: AgentsSub,
}

#[derive(Subcommand, Debug)]
pub enum AgentsSub {
    /// List agents.
    #[command(visible_alias = "list")]
    Ls {
        /// Only the agents of this project.
        #[arg(long, value_name = "PROJECT")]
        project: Option<String>,
        /// Print only each agent's id and the folder it works in.
        #[arg(long)]
        worktrees: bool,
        #[command(flatten)]
        list: ListFlags,
    },
    /// Show one agent.
    Show { agent: String },
    /// Create an agent and print its id last.
    Add(AddAgentArgs),
    /// Delete an agent. With neither --delete-branch nor --keep-branch, a
    /// branch dux created goes with the worktree and one it found is kept.
    #[command(visible_alias = "remove")]
    Rm {
        agent: String,
        /// Also remove the agent's worktree.
        #[arg(long)]
        delete_worktree: bool,
        /// Delete its branch with the worktree. Needs --delete-worktree: git
        /// will not delete a branch a worktree still has checked out.
        #[arg(long, conflicts_with = "keep_branch", requires = "delete_worktree")]
        delete_branch: bool,
        /// Keep its branch.
        #[arg(long)]
        keep_branch: bool,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Stop everything an agent runs.
    Stop {
        agent: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Start an agent.
    Start {
        agent: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// An agent's provider tabs.
    Tabs {
        #[command(subcommand)]
        command: TabsSub,
    },
}

#[derive(Subcommand, Debug)]
pub enum TabsSub {
    /// List an agent's tabs.
    #[command(visible_alias = "list")]
    Ls {
        agent: String,
        #[command(flatten)]
        list: ListFlags,
    },
    /// Add a tab to an agent.
    Add {
        agent: String,
        /// The provider the tab runs (the project's by default).
        #[arg(long, value_name = "NAME")]
        provider: Option<String>,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Close a tab.
    #[command(visible_alias = "remove")]
    Rm {
        agent: String,
        tab: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Start a tab.
    Start {
        agent: String,
        tab: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Stop a tab.
    Stop {
        agent: String,
        tab: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
}

#[derive(Args, Debug)]
pub struct TerminalsCmd {
    #[command(subcommand)]
    pub command: TerminalsSub,
}

#[derive(Subcommand, Debug)]
pub enum TerminalsSub {
    /// List terminals.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Open a terminal and print its id last.
    Add(AddTerminalArgs),
    /// Close a terminal.
    #[command(visible_alias = "remove")]
    Rm {
        terminal: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
}

#[derive(Args, Debug)]
pub struct NamedReadCmd {
    #[command(subcommand)]
    pub command: NamedReadSub,
}

#[derive(Subcommand, Debug)]
pub enum NamedReadSub {
    /// List them.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one by name.
    Show { name: String },
}

#[derive(Args, Debug)]
pub struct MacrosCmd {
    #[command(subcommand)]
    pub command: MacrosSub,
}

#[derive(Subcommand, Debug)]
pub enum MacrosSub {
    /// List the macros.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one macro by name.
    Show { name: String },
    /// Add a macro, or replace the one with that name.
    Add {
        name: String,
        /// The text the macro types.
        text: String,
        /// Where the macro is offered: in agents, terminals, or both.
        #[arg(long, default_value = "agent", value_parser = ["agent", "terminal", "both"])]
        surface: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Remove a macro.
    #[command(visible_alias = "remove")]
    Rm {
        name: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
}

#[derive(Args, Debug)]
pub struct ListOnlyCmd {
    #[command(subcommand)]
    pub command: ListOnlySub,
}

#[derive(Subcommand, Debug)]
pub enum ListOnlySub {
    /// List them.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
}

#[derive(Args, Debug)]
pub struct EnvCmd {
    #[command(subcommand)]
    pub command: EnvSub,
}

#[derive(Subcommand, Debug)]
pub enum EnvSub {
    /// List the variables.
    #[command(visible_alias = "list")]
    Ls {
        /// Print the values too, which may be secrets.
        #[arg(long)]
        show: bool,
        #[command(flatten)]
        list: ListFlags,
    },
    /// Set a variable; the value is asked for, or read from stdin.
    Set {
        name: String,
        /// Read the value from standard input.
        #[arg(long)]
        stdin: bool,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Remove a variable.
    #[command(visible_alias = "remove")]
    Rm {
        name: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
}

#[derive(Args, Debug)]
pub struct RemoteCmd {
    #[command(subcommand)]
    pub command: RemoteSub,
}

#[derive(Subcommand, Debug)]
pub enum RemoteSub {
    /// Save a remote under a name.
    Add {
        name: String,
        url: String,
        /// Allow plain http:// to an address that is neither this machine nor
        /// a Tailscale address. The password then crosses the network
        /// unencrypted, and every login says so.
        #[arg(long)]
        insecure: bool,
    },
    /// List saved remotes.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Forget a saved remote.
    #[command(visible_alias = "remove")]
    Rm { name: String },
    /// Choose, or with --unset clear, the remote used when none is named.
    Default {
        #[arg(required_unless_present = "unset")]
        name: Option<String>,
        /// Clear the default remote.
        #[arg(long, conflicts_with = "name")]
        unset: bool,
    },
    /// Sign in to a remote (the selected one when no name is given).
    Login {
        name: Option<String>,
        /// Read the password from standard input instead of asking for it.
        #[arg(long)]
        stdin: bool,
    },
    /// Sign out of a remote (the selected one when no name is given).
    Logout { name: Option<String> },
}

#[derive(Args, Debug)]
pub struct OperationsCmd {
    #[command(subcommand)]
    pub command: OperationsSub,
}

#[derive(Subcommand, Debug)]
pub enum OperationsSub {
    /// Show where a change stands.
    Show { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("dux").chain(args.iter().copied()))
    }

    fn server(args: &[&str]) -> ServerCmd {
        let mut all = vec!["server"];
        all.extend_from_slice(args);
        match parse(&all).expect("parses").command {
            Some(Command::Server(s)) => s,
            other => panic!("expected server, got {other:?}"),
        }
    }

    fn error_text(args: &[&str]) -> String {
        parse(args).expect_err("must not parse").to_string()
    }

    #[test]
    fn bare_dux_has_no_command() {
        assert!(parse(&[]).unwrap().command.is_none());
    }

    #[test]
    fn server_with_no_flags_has_no_overrides() {
        let o = server(&[]).into_overrides();
        assert!(o.bind.is_none());
        assert!(o.port.is_none());
        assert!(!o.no_tailscale);
    }

    #[test]
    fn server_port_parses_in_both_spellings() {
        assert_eq!(server(&["--port", "9090"]).port, Some(9090));
        assert_eq!(server(&["--port=7000"]).port, Some(7000));
    }

    #[test]
    fn server_port_must_be_a_port_number() {
        assert!(error_text(&["server", "--port", "70000"]).contains("--port"));
    }

    #[test]
    fn server_bind_parses_and_is_refused_twice() {
        assert_eq!(
            server(&["--bind", "0.0.0.0:8888"]).bind.as_deref(),
            Some("0.0.0.0:8888")
        );
        assert!(
            error_text(&["server", "--bind", "a:1", "--bind", "b:2"])
                .contains("cannot be used multiple times")
        );
    }

    #[test]
    fn server_no_tailscale_sets_its_field() {
        assert!(server(&["--no-tailscale"]).no_tailscale);
    }

    #[test]
    fn server_flags_that_were_removed_are_unknown() {
        for flag in [
            "--listen",
            "--disable-auth",
            "--insecure-allow-remote",
            "--acme-domain",
            "--no-acme",
            "--dangerously-listen-http",
        ] {
            assert!(
                error_text(&["server", flag]).contains(flag),
                "{flag} should be named as unknown"
            );
        }
    }

    #[test]
    fn server_flag_without_its_value_is_named() {
        assert!(error_text(&["server", "--bind"]).contains("--bind"));
    }

    #[test]
    fn server_inspection_commands_carry_no_listener_flags() {
        assert!(parse(&["server", "logs", "-f"]).is_ok());
        assert!(parse(&["server", "connections", "ls"]).is_ok());
        let logs = |args: &[&str]| match server(args).command {
            Some(ServerSub::Logs { follow, lines }) => (follow, lines),
            other => panic!("expected logs, got {other:?}"),
        };
        assert_eq!(logs(&["logs"]), (false, 100));
        assert_eq!(logs(&["logs", "-f", "--lines", "5"]), (true, 5));
        assert!(error_text(&["server", "logs", "--lines", "many"]).contains("--lines"));
    }

    #[test]
    fn server_help_carries_the_password_and_shared_workspace_story() {
        let help = error_text(&["server", "--help"]);
        for needle in [
            "password",
            "shares",
            "dux config set server.auth.password",
            "require",
        ] {
            assert!(help.contains(needle), "missing {needle}:\n{help}");
        }
        assert!(!help.contains("no login"), "{help}");
    }

    #[test]
    fn config_arguments_pass_through_untouched() {
        let cli = parse(&[
            "config",
            "set",
            "ui.left_width_pct",
            "-1",
            "--bogus",
            "--help",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Config(c)) => assert_eq!(
                c.args,
                ["set", "ui.left_width_pct", "-1", "--bogus", "--help"]
            ),
            other => panic!("expected config, got {other:?}"),
        }
    }

    #[test]
    fn remote_and_local_exclude_each_other() {
        assert!(parse(&["--remote", "a", "--local", "projects", "ls"]).is_err());
        assert!(parse(&["--local", "projects", "ls"]).is_ok());
        assert!(parse(&["projects", "ls", "--remote", "a"]).is_ok());
    }

    #[test]
    fn ls_and_rm_have_their_aliases() {
        assert!(parse(&["agents", "list"]).is_ok());
        assert!(parse(&["agents", "remove", "a"]).is_ok());
        assert!(parse(&["agents", "tabs", "list", "a"]).is_ok());
    }

    #[test]
    fn remote_default_wants_a_name_or_unset_but_not_both() {
        assert!(parse(&["remote", "default"]).is_err());
        assert!(parse(&["remote", "default", "box"]).is_ok());
        assert!(parse(&["remote", "default", "--unset"]).is_ok());
        assert!(parse(&["remote", "default", "box", "--unset"]).is_err());
    }

    #[test]
    fn global_flags_are_accepted_at_every_depth() {
        for args in [
            &["--remote", "a", "server", "logs"][..],
            &["server", "--remote", "a", "logs"][..],
            &["server", "logs", "--remote", "a"][..],
            &["server", "connections", "ls", "--remote", "a"][..],
            &["agents", "tabs", "ls", "x", "--local"][..],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn listener_flags_are_refused_beside_a_subcommand_but_global_flags_are_not() {
        let with_port = server_cmd(&["server", "--port", "9", "logs"]);
        assert!(listener_flags_with_subcommand(&with_port).is_err());
        let with_remote = server_cmd(&["server", "--remote", "a", "logs"]);
        assert!(listener_flags_with_subcommand(&with_remote).is_ok());
        let plain = server_cmd(&["server", "--port", "9"]);
        assert!(listener_flags_with_subcommand(&plain).is_ok());
    }

    fn server_cmd(args: &[&str]) -> ServerCmd {
        match parse(args).unwrap().command {
            Some(Command::Server(s)) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_selected_remote_refuses_config_unless_local() {
        let msg = "dux config edits this machine's config.toml; run \"dux --local config …\" to go ahead, or unset DUX_REMOTE";
        assert_eq!(
            check_config_target(Some("box"), false),
            Err(msg.to_string())
        );
        assert_eq!(check_config_target(None, false), Ok(()));
        assert_eq!(check_config_target(None, true), Ok(()));
        assert_eq!(check_config_target(Some("box"), true), Ok(()));
    }

    #[test]
    fn resource_changes_parse_and_conflicting_flags_are_refused() {
        for args in [
            &["macros", "add", "m", "hello", "--surface", "both"][..],
            &["macros", "remove", "m"][..],
            &["env", "set", "T", "--stdin"][..],
            &["env", "rm", "T"][..],
            &["projects", "add", "/src/app", "--name", "app", "--init"][..],
            &["projects", "rm", "app", "--delete-worktrees"][..],
            &[
                "agents",
                "add",
                "--project",
                "app",
                "--name",
                "a",
                "--existing-branch",
            ][..],
            &["agents", "add", "--project", "app", "--from-pr", "42"][..],
            &[
                "agents",
                "add",
                "--project",
                "app",
                "--from-worktree",
                "/w/x",
            ][..],
            &[
                "agents",
                "add",
                "--standalone",
                "/src/x",
                "--provider",
                "codex",
            ][..],
            &["agents", "add", "--fork", "a", "--name", "b"][..],
            &["agents", "ls", "--worktrees", "--project", "app"][..],
            &["agents", "rm", "a", "--delete-worktree", "--keep-branch"][..],
            &["agents", "rm", "a", "--keep-branch"][..],
            &[
                "agents",
                "tabs",
                "stop",
                "a",
                "t",
                "--dangerously-ignore-connected",
            ][..],
            &["terminals", "add", "--agent", "a"][..],
            &["terminals", "add"][..],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        for args in [
            &["agents", "add", "--name", "a"][..],
            &["agents", "add", "--fork", "a", "--copy-uncommitted"][..],
            &["agents", "add", "--project", "app", "--fork", "a"][..],
            &["agents", "add", "--from-pr", "42"][..],
            &["agents", "add", "--project", "app", "--provider", "codex"][..],
            &[
                "agents",
                "add",
                "--project",
                "app",
                "--from-pr",
                "42",
                "--existing-branch",
            ][..],
            &["agents", "rm", "a", "--delete-branch", "--keep-branch"][..],
            &["agents", "rm", "a", "--delete-branch"][..],
            &["terminals", "add", "--agent", "a", "--project", "p"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?} should be refused");
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn config_is_split_off_before_clap_sees_what_follows() {
        let split = split_config(&strings(&["config", "--local", "--", "--remote=x"])).unwrap();
        assert_eq!(split.remote, None);
        assert!(!split.local);
        assert_eq!(split.args, ["--local", "--", "--remote=x"]);

        let split = split_config(&strings(&["--remote", "box", "config", "path"])).unwrap();
        assert_eq!(split.remote.as_deref(), Some("box"));
        assert_eq!(split.args, ["path"]);

        let split = split_config(&strings(&["--remote=box", "config"])).unwrap();
        assert_eq!(split.remote.as_deref(), Some("box"));
        assert!(split.args.is_empty());

        let split = split_config(&strings(&["--local", "config", "get", "a"])).unwrap();
        assert!(split.local);
    }

    #[test]
    fn only_a_leading_config_word_is_split_off() {
        assert!(split_config(&strings(&["server", "config"])).is_none());
        assert!(split_config(&strings(&["agents", "config"])).is_none());
        assert!(split_config(&strings(&["--remote"])).is_none());
        assert!(split_config(&strings(&[])).is_none());
        assert!(split_config(&strings(&["--local", "--remote", "a", "config"])).is_none());
    }

    #[test]
    fn a_server_start_refuses_a_selected_target() {
        let msg = "dux server starts a server on this machine; --remote and --local apply to its logs and connections subcommands";
        assert_eq!(
            check_server_start_target(Some("x"), false),
            Err(msg.to_string())
        );
        assert_eq!(check_server_start_target(None, true), Err(msg.to_string()));
        assert_eq!(check_server_start_target(None, false), Ok(()));
    }

    #[test]
    fn a_wait_cannot_be_both_skipped_and_bounded() {
        assert!(parse(&["agents", "rm", "a", "--no-wait", "--wait-timeout", "5"]).is_err());
    }
}
