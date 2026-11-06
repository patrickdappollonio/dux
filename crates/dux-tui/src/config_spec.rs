//! What `dux config` accepts, described for clap so its help and the
//! command-line reference page print it, while the parser in [`crate::cli`]
//! and [`crate::config_cli`] stays hand-written: `dux config` owns every
//! message and exit code of its command, and every word after it.

use clap::{CommandFactory as _, Parser, Subcommand};

/// The flags each `dux config` subcommand accepts, as the parser checks them.
pub(crate) const CONFIG_FLAGS: &[(&str, &[&str])] = &[
    ("path", &[]),
    ("get", &["--show"]),
    ("set", &["--stdin"]),
    ("diff", &["--raw"]),
    ("reset", &["--all"]),
    ("regenerate", &["--yes", "--show"]),
    ("restore-docs", &["--yes", "--show"]),
];

/// The flags `subcommand` accepts; none for a word that is no subcommand.
pub(crate) fn config_flags(subcommand: &str) -> &'static [&'static str] {
    CONFIG_FLAGS
        .iter()
        .find(|(name, _)| *name == subcommand)
        .map_or(&[], |(_, flags)| flags)
}

#[derive(Parser, Debug)]
#[command(
    name = "config",
    bin_name = "dux config",
    about = "Manage the dux configuration file",
    disable_help_flag = true,
    disable_help_subcommand = true
)]
#[expect(dead_code, reason = "described for its help, never parsed into")]
struct ConfigSpec {
    #[command(subcommand)]
    command: ConfigSub,
}

#[derive(Subcommand, Debug)]
#[command(disable_help_flag = true)]
#[expect(dead_code, reason = "described for its help, never parsed into")]
enum ConfigSub {
    /// Print the config file path
    Path,
    /// Print one setting's value, for example `dux config get server.port`
    Get {
        /// The setting's dotted name, for example `server.port`
        setting: String,
        /// Values that can hold secrets (env, projects) need --show
        #[arg(long)]
        show: bool,
    },
    /// Change one setting, keeping the file's comments, and have a running
    /// dux reload, saying whether it worked (exit 1 if it refused or half
    /// applied, 6 if it gave no answer in time)
    ///
    /// `dux config set server.auth.password` sets the web UI password: asked
    /// for twice without echo, with a strength meter. Never a command-line
    /// argument; add --stdin to pipe it in instead. env.<NAME> values are
    /// asked for the same way.
    Set {
        /// The setting's dotted name, for example `server.port`
        setting: String,
        /// The new value. Lists are one TOML array: `'["a", "b"]'`. A value that
        /// starts with two dashes goes after `--`. Left out for the password
        /// and env.<NAME> values, which are asked for
        value: Option<String>,
        /// Pipe the password or an env.<NAME> value in instead of being asked
        /// for it
        #[arg(long)]
        stdin: bool,
    },
    /// Show settings that differ from defaults (summary; [env] and project
    /// details are summarized, never printed, so it is safe to paste into a
    /// bug report)
    Diff {
        /// Show a unified diff against the default config. This prints the
        /// WHOLE config, [env] values included: redact it before sharing.
        #[arg(long)]
        raw: bool,
    },
    /// Remove config, logs and saved remotes (keeps agents and worktrees)
    Reset {
        /// Full factory reset: remove config, logs, saved remotes, sessions,
        /// and worktrees (deletes nothing if a program dux started still runs)
        #[arg(long)]
        all: bool,
    },
    /// Preview a fresh default config (shows diff; [env] and other sensitive
    /// values are hidden unless you add --show)
    Regenerate {
        /// Overwrite the config file with fresh defaults
        #[arg(long)]
        yes: bool,
        /// Show [env] and other sensitive values in the preview
        #[arg(long)]
        show: bool,
    },
    /// Preview re-adding the explanatory comments to your config, keeping
    /// every value you have set (hides values like regenerate; --show reveals
    /// them)
    RestoreDocs {
        /// Apply it (writes a timestamped backup first)
        #[arg(long)]
        yes: bool,
        /// Show the hidden values in the preview
        #[arg(long)]
        show: bool,
    },
}

/// `dux config`'s description, for its help and the reference page. Nothing
/// parses with it.
#[must_use]
pub fn config_command() -> clap::Command {
    ConfigSpec::command()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn the_flag_table_names_the_subcommands_and_flags_the_description_does() {
        let described: BTreeSet<(String, BTreeSet<String>)> = config_command()
            .get_subcommands()
            .map(|sub| {
                let flags = sub
                    .get_arguments()
                    .filter_map(|arg| arg.get_long())
                    .map(|long| format!("--{long}"))
                    .collect();
                (sub.get_name().to_string(), flags)
            })
            .collect();
        let parsed: BTreeSet<(String, BTreeSet<String>)> = CONFIG_FLAGS
            .iter()
            .map(|(name, flags)| {
                let flags = flags.iter().map(ToString::to_string).collect();
                ((*name).to_string(), flags)
            })
            .collect();
        assert_eq!(described, parsed);
    }
}
