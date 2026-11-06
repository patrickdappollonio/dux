//! The one command tree of the `dux` binary. Everything the command line
//! accepts is declared here and parsed before a lock is taken or an engine
//! starts; `main` only dispatches on the result.

use std::fmt::Write as _;

use clap::{Args, Parser, Subcommand, ValueEnum};
use dux_core::client::connect::LocalFlag;

const SERVER_ABOUT: &str = "\
Run the dux web UI without the terminal UI. The web UI has one optional
password for one owner, and everyone who gets in shares the one workspace. Set
it with `dux config set server.auth.password`; [server.auth] require decides
who is asked for it (by default everyone but this machine and your tailnet).
With no password, anyone who can reach a non-loopback address controls your
agents and terminals, and dux says so loudly as it starts.";

/// An environment variable dux reads, as `dux --help` and the reference page
/// list it.
#[derive(Debug)]
pub(crate) struct EnvironmentVariable {
    pub name: &'static str,
    /// The description, one entry per line `--help` prints. An entry that
    /// starts with spaces is an item of a list, which the reference page puts
    /// on a line of its own; every other entry continues the sentence before.
    pub lines: &'static [&'static str],
}

pub(crate) const ENVIRONMENT: &[EnvironmentVariable] = &[
    EnvironmentVariable {
        name: "DUX_HOME",
        lines: &[
            "Override the config directory (must be an absolute path).",
            "When unset, defaults to:",
            "  macOS: ~/.dux/",
            "  Linux: $XDG_CONFIG_HOME/dux/ or ~/.config/dux/",
        ],
    },
    EnvironmentVariable {
        name: "DUX_REMOTE",
        lines: &[
            "The saved remote to talk to when neither --remote nor --local",
            "is given. It wins over the default set with `dux remote default`.",
        ],
    },
];

/// Every exit code a command can end with and what it means, success first.
pub(crate) fn exit_codes() -> Vec<(i32, &'static str)> {
    std::iter::once((0, "success"))
        .chain(
            dux_core::client::Exit::ALL
                .iter()
                .map(|exit| (exit.code(), exit.meaning())),
        )
        .collect()
}

/// The widest line of an exit code's row in `dux --help`.
const EXIT_ROW_WIDTH: usize = 77;

/// What `dux --help` prints after the commands: the environment variables,
/// where state lives, and the exit codes.
fn root_after_help() -> String {
    let mut out = String::from("Environment variables:\n");
    let column = ENVIRONMENT
        .iter()
        .map(|var| var.name.len())
        .max()
        .unwrap_or(0);
    for var in ENVIRONMENT {
        for (index, line) in var.lines.iter().enumerate() {
            let name = if index == 0 { var.name } else { "" };
            let _ = writeln!(out, "  {name:<column$}  {line}");
        }
    }
    out.push_str(
        "\n\
First run writes a full default config to config.toml in that directory, and
session state is stored beside it in sessions.sqlite3.

Projects, agents, terminals, `dux server connections` and `dux operations`
need a running dux: the one on this machine, or a saved remote. Macros,
providers, keys, themes and env read and edit config.toml when dux is stopped,
and `dux server logs` reads server.log. These commands end with these exit
codes:",
    );
    for (code, meaning) in exit_codes() {
        let mut line = format!("  {code} ");
        for word in meaning.split(' ') {
            if line.len() + 1 + word.len() > EXIT_ROW_WIDTH {
                out.push('\n');
                out.push_str(&line);
                line = "    ".to_string();
            }
            line.push(' ');
            line.push_str(word);
        }
        out.push('\n');
        out.push_str(&line);
    }
    out
}

#[derive(Parser, Debug)]
#[command(
    name = "dux",
    version,
    about = "Terminal and web UI for AI worktree sessions, and a command line for their state. Run with no command to launch the terminal UI.",
    after_help = root_after_help()
)]
pub(crate) struct Cli {
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
pub(crate) enum Command {
    /// Serve the web UI without the terminal UI, or inspect a running server.
    Server(ServerCmd),
    /// Manage the configuration file on this machine.
    // The help of its subcommands lives in crates/dux-tui/src/config_spec.rs.
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
    Providers(ProvidersCmd),
    /// Keybindings from config.toml.
    Keys(KeysCmd),
    /// Themes dux can load.
    Themes(ThemesCmd),
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
pub(crate) struct ServerCmd {
    /// Bind this exact address, overriding [server] host and port. An IP:port
    /// address (hostnames are NOT resolved), e.g. 0.0.0.0:3890.
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
    pub(crate) fn into_overrides(self) -> dux_core::config::ServerCliOverrides {
        dux_core::config::ServerCliOverrides {
            bind: self.bind,
            port: self.port,
            no_tailscale: self.no_tailscale,
        }
    }
}

/// The listener flags only make sense when starting a server, so they are
/// refused beside `logs` and `connections`. The global flags are not.
pub(crate) fn listener_flags_with_subcommand(server: &ServerCmd) -> Result<(), String> {
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
pub(crate) fn check_server_start_target(
    remote: Option<&str>,
    local: LocalFlag,
) -> Result<(), String> {
    if remote.is_some() || local == LocalFlag::Given {
        return Err("dux server starts a server on this machine; --remote and --local apply to its logs and connections subcommands".to_string());
    }
    Ok(())
}

/// Starting dux with no command starts the terminal UI on this machine, so
/// naming a target is a mistake.
pub(crate) fn check_terminal_ui_start_target(
    remote: Option<&str>,
    local: LocalFlag,
) -> Result<(), String> {
    if remote.is_some() || local == LocalFlag::Given {
        return Err("dux with no command starts the terminal UI on this machine and takes no target; --remote and --local apply to the commands that talk to a dux".to_string());
    }
    Ok(())
}

/// What `dux [--remote x | --local] config <args>` carries: the global flags
/// that came before the word, and everything after it, byte for byte.
#[derive(Debug)]
pub(crate) struct ConfigInvocation {
    pub remote: Option<String>,
    pub local: bool,
    pub args: Vec<String>,
}

/// Splits off a command line whose command is `config`, after the global flags alone, so clap
/// never sees its words. Any other shape, or one naming both global flags, is left to clap.
pub(crate) fn split_config(args: &[String]) -> Option<ConfigInvocation> {
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

/// What selected the remote a command would talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteSource {
    /// `--remote` on the command line.
    Flag,
    /// The `DUX_REMOTE` environment variable.
    Variable,
    /// The default saved with `dux remote default`.
    SavedDefault,
}

/// `dux config` edits this machine's file, so a selected `remote` is refused unless
/// `--local` is given, naming what selected it and the way out that applies.
pub(crate) fn check_config_target(
    remote: Option<(&str, RemoteSource)>,
    local: LocalFlag,
) -> Result<(), String> {
    let Some((name, source)) = remote else {
        return Ok(());
    };
    if local == LocalFlag::Given {
        return Ok(());
    }
    let start = "dux config edits this machine's config.toml";
    Err(match source {
        RemoteSource::Flag => {
            format!("{start}, and --remote {name} names another machine; run it without --remote")
        }
        RemoteSource::Variable => format!(
            "{start}, and DUX_REMOTE selects {name}; run \"dux --local config …\" to go ahead, \
             or unset DUX_REMOTE"
        ),
        RemoteSource::SavedDefault => format!(
            "{start}, and {name} is the saved default remote; run \"dux --local config …\" to \
             go ahead, or \"dux remote default --unset\" to clear the default"
        ),
    })
}

#[derive(Subcommand, Debug)]
pub(crate) enum ServerSub {
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
pub(crate) enum ConnectionsSub {
    /// List the connections.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
}

/// `dux config` hands its arguments, untouched, to the config code, which
/// owns every message and exit code of that command.
#[derive(Args, Debug)]
pub(crate) struct ConfigCmd {
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
pub(crate) enum Format {
    Table,
    Json,
}

#[derive(Args, Debug)]
pub(crate) struct ListFlags {
    /// How to print the list: `table` to read, `json` for a script.
    #[arg(long, value_enum, default_value = "table")]
    pub format: Format,
    /// Print only the ids, one per line.
    #[arg(short = 'q', long)]
    pub quiet: bool,
}

/// What every change takes: whether to confirm, and how long to wait.
#[derive(Args, Debug)]
pub(crate) struct ChangeFlags {
    /// Do not ask for confirmation (required without a terminal).
    #[arg(long)]
    pub yes: bool,
    /// Print the operation id and return without waiting for the change.
    #[arg(long)]
    pub no_wait: bool,
    /// Wait at most this many seconds for the change to finish (at most a day).
    /// Not with --no-wait.
    #[arg(long, value_name = "SECONDS", conflicts_with = "no_wait")]
    pub wait_timeout: Option<u64>,
}

impl ChangeFlags {
    /// Whether `--yes` already gave the confirmation.
    pub(crate) fn consent(&self) -> dux_core::client::output::Consent {
        if self.yes {
            dux_core::client::output::Consent::Given
        } else {
            dux_core::client::output::Consent::Ask
        }
    }
}

/// A change that can cut off whoever is attached to what it touches.
#[derive(Args, Debug)]
pub(crate) struct GuardedChangeFlags {
    #[command(flatten)]
    pub change: ChangeFlags,
    /// Go ahead even though someone is attached.
    #[arg(long)]
    pub dangerously_ignore_connected: bool,
}

/// `dux agents add`: one of a project, `--fork`, `--standalone` or a pull
/// request says where the agent comes from. A pull request may also come with
/// a project, which then names where it goes.
#[derive(Args, Debug)]
#[command(group(
    clap::ArgGroup::new("source")
        .args(["project", "fork", "standalone", "from_pr"])
        .required(true)
        .multiple(true)
))]
pub(crate) struct AddAgentArgs {
    /// The project to create the agent in, on a new branch unless
    /// --from-pr or --from-worktree says otherwise. Not with --fork or
    /// --standalone.
    #[arg(long, value_name = "PROJECT", conflicts_with_all = ["fork", "standalone"])]
    pub project: Option<String>,
    /// The agent's name, which is also its branch's name in a project.
    #[arg(long)]
    pub name: Option<String>,
    /// Create the agent on the branch of that name that already exists. Not
    /// with --from-pr, --from-worktree, --fork or --standalone.
    #[arg(long, conflicts_with_all = ["from_pr", "from_worktree", "fork", "standalone"])]
    pub existing_branch: bool,
    /// Create the agent from this pull request: a number or its address with
    /// --project, or an address or owner/repo#123 alone, which clones the
    /// repository (or adds a checkout of it) when no project has it. Not with
    /// --from-worktree, --fork or --standalone.
    #[arg(
        long,
        value_name = "PR",
        conflicts_with_all = ["from_worktree", "fork", "standalone"]
    )]
    pub from_pr: Option<String>,
    /// The folder to clone the pull request's repository into, or the existing
    /// checkout of it to add as a project, when no project has it. By default
    /// the server's project-browser start folder plus the repository's name.
    /// Needs --from-pr, and not with --project.
    #[arg(
        long,
        value_name = "FOLDER",
        requires = "from_pr",
        conflicts_with = "project"
    )]
    pub path: Option<String>,
    /// Agree to a separate review copy of the pull request when its own branch
    /// cannot be used as it is (it has diverged, is checked out elsewhere, or
    /// could not be compared). Without this such a create is refused and
    /// nothing is made, because pushes from a copy land on the copy's branch
    /// and not on the pull request. Needs --from-pr.
    #[arg(long, requires = "from_pr")]
    pub review_copy: bool,
    /// Create the agent on this worktree the project already has. Needs
    /// --project.
    #[arg(long, value_name = "PATH", requires = "project")]
    pub from_worktree: Option<String>,
    /// Create a standalone agent in this folder of your own, with no project.
    /// Not with --fork.
    #[arg(long, value_name = "FOLDER", conflicts_with = "fork")]
    pub standalone: Option<String>,
    /// The provider a standalone agent runs (the global default otherwise).
    /// Not with --project, --fork or --from-pr.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["project", "fork", "from_pr"])]
    pub provider: Option<String>,
    /// Fork this agent: a new worktree from its branch.
    #[arg(long, value_name = "AGENT")]
    pub fork: Option<String>,
    /// Copy the uncommitted changes of the project's checkout into the new
    /// worktree. Not with --from-pr, --from-worktree, --fork or --standalone.
    #[arg(long, conflicts_with_all = ["from_pr", "from_worktree", "fork", "standalone"])]
    pub copy_uncommitted: bool,
    #[command(flatten)]
    pub change: ChangeFlags,
}

/// `dux terminals add`: an agent's or a project's, or a standalone one when
/// neither is named.
#[derive(Args, Debug)]
pub(crate) struct AddTerminalArgs {
    /// Open it in this agent's worktree. Not with --project.
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
pub(crate) struct ProjectsCmd {
    #[command(subcommand)]
    pub command: ProjectsSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ProjectsSub {
    /// List projects.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one project.
    Show {
        /// A project's id or name; a name two projects share is refused with both ids.
        project: String,
    },
    /// Add a project from a path and print its id last.
    Add {
        /// The folder of the repository to add, on the machine dux runs on.
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
        /// A project's id or name; a name two projects share is refused with both ids.
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
pub(crate) enum WorktreesSub {
    /// List a project's worktrees.
    #[command(visible_alias = "list")]
    Ls {
        /// A project's id or name; a name two projects share is refused with both ids.
        project: String,
        #[command(flatten)]
        list: ListFlags,
    },
}

#[derive(Args, Debug)]
pub(crate) struct AgentsCmd {
    #[command(subcommand)]
    pub command: AgentsSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum AgentsSub {
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
    Show {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
    },
    /// Create an agent and print its id last.
    Add(AddAgentArgs),
    /// Delete an agent. With no branch flag, only the branch dux made for the
    /// agent goes with the worktree, and only while dux can prove it is still
    /// that branch; every other branch is kept.
    #[command(visible_alias = "remove")]
    Rm {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// Also remove the agent's worktree.
        #[arg(long)]
        delete_worktree: bool,
        /// Delete the branch its worktree is on, at the commit it points at
        /// now. Needs --delete-worktree: git will not delete a branch a
        /// worktree still has checked out. Not with --keep-branch.
        #[arg(long, conflicts_with = "keep_branch", requires = "delete_worktree")]
        delete_branch: bool,
        /// Keep the branch its worktree is on. With --delete-worktree, the
        /// other branches it has been on are still decided as the delete
        /// dialog would tick them, unless --keep-other-branches or
        /// --delete-other-branches says otherwise.
        #[arg(long)]
        keep_branch: bool,
        /// Delete every other branch its worktree has been on, each at the
        /// commit it points at now. Needs --delete-worktree, and not with
        /// --keep-other-branches.
        #[arg(
            long,
            conflicts_with = "keep_other_branches",
            requires = "delete_worktree"
        )]
        delete_other_branches: bool,
        /// Keep every other branch its worktree has been on. With
        /// --delete-worktree, the branch it is on now is still decided as the
        /// delete dialog would tick it, unless --keep-branch or
        /// --delete-branch says otherwise.
        #[arg(long)]
        keep_other_branches: bool,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Stop everything an agent runs.
    Stop {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Start an agent.
    Start {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// An agent's provider tabs.
    Tabs {
        #[command(subcommand)]
        command: TabsSub,
    },
    /// An agent's pull requests: the ones dux found on the branches its
    /// worktree has been on, and the ones you attached from any repository.
    Prs {
        #[command(subcommand)]
        command: PrsSub,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum PrsSub {
    /// List an agent's pull requests in its order, the one its banner shows
    /// first, then the hidden ones. Each row's id is the pull request's link,
    /// which every other `prs` command accepts.
    #[command(visible_alias = "list")]
    Ls {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        #[command(flatten)]
        list: ListFlags,
    },
    /// Attach a pull request to an agent, beside the ones it has. It can come
    /// from any repository, and stays until you detach it.
    Attach {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// The pull request: its link, `owner/repo#123`, or just `123` (or
        /// `'#123'`, quoted, since a shell reads a word starting with `#` as a
        /// comment) in the agent's own repository.
        #[arg(value_name = "REF")]
        reference: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Detach a pull request you attached. One dux also found on the agent's
    /// branches stays in the list, as found.
    Detach {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// One of the agent's pull requests: its link, `owner/repo#123`, or
        /// just `123` (or `'#123'`, quoted, since a shell reads a word starting
        /// with `#` as a comment); a number two of them share is refused with
        /// both links.
        #[arg(value_name = "REF")]
        reference: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Hide a pull request dux found, until you unhide it. It keeps its place
    /// in your own order.
    Hide {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// One of the agent's pull requests: its link, `owner/repo#123`, or
        /// just `123` (or `'#123'`, quoted, since a shell reads a word starting
        /// with `#` as a comment); a number two of them share is refused with
        /// both links.
        #[arg(value_name = "REF")]
        reference: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Show a hidden pull request again, in the place it had.
    Unhide {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// One of the agent's pull requests: its link, `owner/repo#123`, or
        /// just `123` (or `'#123'`, quoted, since a shell reads a word starting
        /// with `#` as a comment); a number two of them share is refused with
        /// both links.
        #[arg(value_name = "REF")]
        reference: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Choose how an agent's pull requests are ordered. Your own order is kept
    /// while the order is automatic, so choosing `manual` again brings it back.
    Sort {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// `auto` (or `automatic`): open, draft, merged, closed, newest first.
        /// `manual`: your own order, with pull requests found later after the
        /// ones you placed.
        #[arg(value_enum)]
        mode: PrSortArg,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Move one of an agent's shown pull requests in its order. A move made
    /// while the order is automatic switches to your own order, starting from
    /// the order it was in, and replaces any order you had saved.
    Move {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// One of the agent's pull requests: its link, `owner/repo#123`, or
        /// just `123` (or `'#123'`, quoted, since a shell reads a word starting
        /// with `#` as a comment); a number two of them share is refused with
        /// both links.
        #[arg(value_name = "REF")]
        reference: String,
        /// Where to: `top`, `up`, `down`, or a position counted from 1.
        #[arg(value_name = "TO", value_parser = parse_move_target)]
        to: dux_core::wire::PullRequestMove,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// List every branch an agent's worktree has been on, the one checked out
    /// now first and the rest newest first, with the names each had before a
    /// rename and its pull requests, then the attached pull requests that are
    /// on none of them.
    Branches {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        #[command(flatten)]
        list: ListFlags,
    },
}

/// The orders `dux agents prs sort` chooses between.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum PrSortArg {
    /// Open, draft, merged, closed, newest first.
    #[value(alias = "automatic")]
    Auto,
    /// Your own order.
    Manual,
}

impl From<PrSortArg> for dux_core::wire::PullRequestSort {
    fn from(arg: PrSortArg) -> Self {
        match arg {
            PrSortArg::Auto => Self::Automatic,
            PrSortArg::Manual => Self::Manual,
        }
    }
}

/// `top`, `up`, `down`, or a position counted from 1.
fn parse_move_target(text: &str) -> Result<dux_core::wire::PullRequestMove, String> {
    use dux_core::wire::{PullRequestMove, PullRequestMoveNamed};
    match text {
        "top" => Ok(PullRequestMove::Named(PullRequestMoveNamed::Top)),
        "up" => Ok(PullRequestMove::Named(PullRequestMoveNamed::Up)),
        "down" => Ok(PullRequestMove::Named(PullRequestMoveNamed::Down)),
        _ => match text.parse::<usize>() {
            Ok(position) if position >= 1 => Ok(PullRequestMove::Position(position)),
            _ => Err("use top, up, down, or a position counted from 1".to_string()),
        },
    }
}

#[derive(Subcommand, Debug)]
pub(crate) enum TabsSub {
    /// List an agent's tabs.
    #[command(visible_alias = "list")]
    Ls {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        #[command(flatten)]
        list: ListFlags,
    },
    /// Add a tab to an agent.
    Add {
        /// An agent's id or name; a name two agents share is refused with both ids.
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
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// A tab's id or the label the tab strip shows it under.
        tab: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
    /// Start a tab.
    Start {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// A tab's id or the label the tab strip shows it under.
        tab: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
    /// Stop a tab.
    Stop {
        /// An agent's id or name; a name two agents share is refused with both ids.
        agent: String,
        /// A tab's id or the label the tab strip shows it under.
        tab: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
}

#[derive(Args, Debug)]
pub(crate) struct TerminalsCmd {
    #[command(subcommand)]
    pub command: TerminalsSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum TerminalsSub {
    /// List terminals.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Open a terminal and print its id last.
    Add(AddTerminalArgs),
    /// Close a terminal.
    #[command(visible_alias = "remove")]
    Rm {
        /// A terminal's id or its name in the list; a name two terminals share is
        /// refused with both ids.
        terminal: String,
        #[command(flatten)]
        guarded: GuardedChangeFlags,
    },
}

#[derive(Args, Debug)]
pub(crate) struct ProvidersCmd {
    #[command(subcommand)]
    pub command: ProvidersSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ProvidersSub {
    /// List the providers, built-in and your own.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one provider and the command it runs.
    Show {
        /// The provider's name, as `dux providers ls` lists it.
        name: String,
    },
}

#[derive(Args, Debug)]
pub(crate) struct MacrosCmd {
    #[command(subcommand)]
    pub command: MacrosSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum MacrosSub {
    /// List the macros.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
    /// Show one macro by name.
    Show {
        /// The macro's name, as `dux macros ls` lists it.
        name: String,
    },
    /// Add a macro, or replace the one with that name.
    Add {
        /// The macro's name; an existing macro with this name is replaced.
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
        /// The macro's name, as `dux macros ls` lists it.
        name: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
}

#[derive(Args, Debug)]
pub(crate) struct KeysCmd {
    #[command(subcommand)]
    pub command: KeysSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum KeysSub {
    /// List every action the terminal UI binds and the keys it uses, yours or
    /// the default.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
}

#[derive(Args, Debug)]
pub(crate) struct ThemesCmd {
    #[command(subcommand)]
    pub command: ThemesSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ThemesSub {
    /// List the themes dux can load, marking the one ui.theme names.
    #[command(visible_alias = "list")]
    Ls(ListFlags),
}

#[derive(Args, Debug)]
pub(crate) struct EnvCmd {
    #[command(subcommand)]
    pub command: EnvSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum EnvSub {
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
        /// The variable's name, for example `GITHUB_TOKEN`.
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
        /// The variable's name, as `dux env ls` lists it.
        name: String,
        #[command(flatten)]
        change: ChangeFlags,
    },
}

#[derive(Args, Debug)]
pub(crate) struct RemoteCmd {
    #[command(subcommand)]
    pub command: RemoteSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum RemoteSub {
    /// Save a remote under a name.
    Add {
        /// The name to save the remote under, used with --remote.
        name: String,
        /// The address of the remote dux's web UI, for example
        /// `https://box.tailnet-name.ts.net`.
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
    Rm {
        /// The saved remote's name, as `dux remote ls` lists it.
        name: String,
    },
    /// Choose, or with --unset clear, the remote used when none is named.
    Default {
        /// The saved remote's name, as `dux remote ls` lists it. Needed unless
        /// --unset is given.
        #[arg(required_unless_present = "unset")]
        name: Option<String>,
        /// Clear the default remote. Not with a name.
        #[arg(long, conflicts_with = "name")]
        unset: bool,
    },
    /// Sign in to a remote (the selected one when no name is given).
    Login {
        /// The saved remote's name (the selected one when left out).
        name: Option<String>,
        /// Read the password from standard input instead of asking for it.
        #[arg(long)]
        stdin: bool,
    },
    /// Sign out of a remote (the selected one when no name is given).
    Logout {
        /// The saved remote's name (the selected one when left out).
        name: Option<String>,
    },
}

#[derive(Args, Debug)]
pub(crate) struct OperationsCmd {
    #[command(subcommand)]
    pub command: OperationsSub,
}

#[derive(Subcommand, Debug)]
pub(crate) enum OperationsSub {
    /// Show where a change stands.
    Show {
        /// The operation id a change printed when it started.
        id: String,
    },
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
    fn top_level_help_ends_with_the_variables_and_the_exit_codes_word_for_word() {
        use clap::CommandFactory as _;
        let after = Cli::command()
            .get_after_help()
            .map(ToString::to_string)
            .expect("after help");
        assert_eq!(
            after,
            "\
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
  6  the outcome is unknown: the wait ran out, and the change keeps running"
        );
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
        assert!(parse(&["agents", "prs", "list", "a"]).is_ok());
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
            &["agents", "prs", "--remote", "a", "ls", "x"][..],
            &["agents", "prs", "branches", "x", "--local"][..],
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
        let default = Some(("box", RemoteSource::SavedDefault));
        assert_eq!(
            check_config_target(default, LocalFlag::Absent),
            Err(
                "dux config edits this machine's config.toml, and box is the saved default \
                 remote; run \"dux --local config …\" to go ahead, or \"dux remote default \
                 --unset\" to clear the default"
                    .to_string()
            )
        );
        assert_eq!(check_config_target(None, LocalFlag::Absent), Ok(()));
        assert_eq!(check_config_target(None, LocalFlag::Given), Ok(()));
        assert_eq!(check_config_target(default, LocalFlag::Given), Ok(()));
    }

    /// Change lines the command tree accepts.
    const ACCEPTED_CHANGES: &[&[&str]] = &[
        &["macros", "add", "m", "hello", "--surface", "both"],
        &["macros", "remove", "m"],
        &["env", "set", "T", "--stdin"],
        &["env", "rm", "T"],
        &["projects", "add", "/src/app", "--name", "app", "--init"],
        &["projects", "rm", "app", "--delete-worktrees"],
        &[
            "agents",
            "add",
            "--project",
            "app",
            "--name",
            "a",
            "--existing-branch",
        ],
        &["agents", "add", "--project", "app", "--from-pr", "42"],
        &["agents", "add", "--from-pr", "42"],
        &[
            "agents",
            "add",
            "--from-pr",
            "octo/app#42",
            "--path",
            "/src/app",
        ],
        &["agents", "add", "--from-pr", "octo/app#42", "--review-copy"],
        &[
            "agents",
            "add",
            "--project",
            "app",
            "--from-pr",
            "42",
            "--review-copy",
        ],
        &[
            "agents",
            "add",
            "--project",
            "app",
            "--from-worktree",
            "/w/x",
        ],
        &[
            "agents",
            "add",
            "--standalone",
            "/src/x",
            "--provider",
            "codex",
        ],
        &["agents", "add", "--fork", "a", "--name", "b"],
        &["agents", "ls", "--worktrees", "--project", "app"],
        &["agents", "rm", "a", "--delete-worktree", "--keep-branch"],
        &["agents", "rm", "a", "--keep-branch"],
        &[
            "agents",
            "rm",
            "a",
            "--delete-worktree",
            "--delete-branch",
            "--delete-other-branches",
        ],
        &["agents", "rm", "a", "--keep-other-branches"],
        &[
            "agents",
            "tabs",
            "stop",
            "a",
            "t",
            "--dangerously-ignore-connected",
        ],
        &["terminals", "add", "--agent", "a"],
        &["terminals", "add"],
        &["agents", "prs", "sort", "a", "automatic"],
        &["agents", "prs", "sort", "a", "auto", "--yes"],
        &["agents", "prs", "move", "a", "418", "2"],
        &["agents", "prs", "move", "a", "#418", "top", "--no-wait"],
    ];

    /// Change lines whose flags conflict, which the command tree refuses.
    const REFUSED_CHANGES: &[&[&str]] = &[
        &["agents", "add", "--name", "a"],
        &["agents", "add", "--fork", "a", "--copy-uncommitted"],
        &["agents", "add", "--project", "app", "--fork", "a"],
        &["agents", "add", "--from-pr", "42", "--fork", "a"],
        &["agents", "add", "--from-pr", "42", "--standalone", "/src/x"],
        &["agents", "add", "--from-pr", "42", "--provider", "codex"],
        &["agents", "add", "--path", "/src/app"],
        &["agents", "add", "--project", "app", "--path", "/src/app"],
        &[
            "agents",
            "add",
            "--project",
            "app",
            "--from-pr",
            "42",
            "--path",
            "/src/app",
        ],
        &["agents", "add", "--project", "app", "--review-copy"],
        &["agents", "add", "--project", "app", "--provider", "codex"],
        &[
            "agents",
            "add",
            "--project",
            "app",
            "--from-pr",
            "42",
            "--existing-branch",
        ],
        &["agents", "rm", "a", "--delete-branch", "--keep-branch"],
        &["agents", "rm", "a", "--delete-branch"],
        &[
            "agents",
            "rm",
            "a",
            "--delete-worktree",
            "--delete-other-branches",
            "--keep-other-branches",
        ],
        &["agents", "rm", "a", "--delete-other-branches"],
        &["terminals", "add", "--agent", "a", "--project", "p"],
        &["agents", "prs", "move", "a", "418", "0"],
        &["agents", "prs", "move", "a", "418", "sideways"],
        &["agents", "prs", "sort", "a", "sideways"],
    ];

    #[test]
    fn resource_changes_parse_and_conflicting_flags_are_refused() {
        for args in ACCEPTED_CHANGES {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        for args in REFUSED_CHANGES {
            assert!(parse(args).is_err(), "{args:?} should be refused");
        }
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(std::string::ToString::to_string).collect()
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
        assert_eq!(split.args, Vec::<String>::new());

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
            check_server_start_target(Some("x"), LocalFlag::Absent),
            Err(msg.to_string())
        );
        assert_eq!(
            check_server_start_target(None, LocalFlag::Given),
            Err(msg.to_string())
        );
        assert_eq!(check_server_start_target(None, LocalFlag::Absent), Ok(()));
    }

    #[test]
    fn starting_the_terminal_ui_refuses_a_selected_target() {
        let msg = "dux with no command starts the terminal UI on this machine and takes no target; --remote and --local apply to the commands that talk to a dux";
        assert_eq!(
            check_terminal_ui_start_target(Some("box"), LocalFlag::Absent),
            Err(msg.to_string())
        );
        assert_eq!(
            check_terminal_ui_start_target(None, LocalFlag::Given),
            Err(msg.to_string())
        );
        assert_eq!(
            check_terminal_ui_start_target(None, LocalFlag::Absent),
            Ok(())
        );
    }

    #[test]
    fn a_wait_cannot_be_both_skipped_and_bounded() {
        assert!(parse(&["agents", "rm", "a", "--no-wait", "--wait-timeout", "5"]).is_err());
    }
}
