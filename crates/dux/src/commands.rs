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

First run writes a full default config to config.toml in that directory, and
session state is stored beside it in sessions.sqlite3.";

#[derive(Parser, Debug)]
#[command(
    name = "dux",
    version,
    about = "Terminal and web UI for AI worktree sessions. Run with no command to launch the terminal UI.",
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
    Macros(NamedReadCmd),
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
#[command(before_help = SERVER_ABOUT, args_conflicts_with_subcommands = true)]
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

#[derive(Subcommand, Debug)]
pub enum ServerSub {
    /// Show the server's log.
    Logs {
        /// Keep printing new lines as they are written.
        #[arg(short = 'f', long)]
        follow: bool,
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

/// A command whose arguments a later part of the command line defines.
#[derive(Args, Debug)]
pub struct OpenArgs {
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        value_name = "ARGS"
    )]
    pub args: Vec<String>,
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
    /// Add a project from a path.
    Add {
        path: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Remove a project.
    #[command(visible_alias = "remove")]
    Rm {
        project: String,
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
        #[command(flatten)]
        list: ListFlags,
    },
    /// Show one agent.
    Show { agent: String },
    /// Create an agent.
    Add(OpenArgs),
    /// Delete an agent.
    #[command(visible_alias = "remove")]
    Rm {
        agent: String,
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
    /// Open a terminal.
    Add(OpenArgs),
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
}

#[derive(Args, Debug)]
pub struct RemoteCmd {
    #[command(subcommand)]
    pub command: RemoteSub,
}

#[derive(Subcommand, Debug)]
pub enum RemoteSub {
    /// Save a remote under a name.
    Add { name: String, url: String },
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
    /// Sign in to a remote.
    Login { name: Option<String> },
    /// Sign out of a remote.
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
        assert!(parse(&["server", "--port", "9", "logs"]).is_err());
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
    fn a_wait_cannot_be_both_skipped_and_bounded() {
        assert!(parse(&["agents", "rm", "a", "--no-wait", "--wait-timeout", "5"]).is_err());
    }
}
