//! `dux config get <path>` and `dux config set <path> [value]`.
//!
//! Thin over [`dux_core::config_keys`], which owns what a path means, how a
//! value is checked and how it is written (comment-keeping, under the config
//! file's write lock). This module owns the command line: argument parsing,
//! the hidden password prompt with its live strength line, and telling a
//! running dux to reload.
//!
//! Neither command takes the single-instance lock: `get` only reads, and
//! `set` is meant to run beside a live dux. Its write takes the config file's
//! own lock, and it then asks the dux holding `dux.lock` to reload over its
//! control socket and prints how that went (see
//! [`dux_core::client::reload`]).

use std::io::{Read, Write};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use dux_core::auth::{Password, PasswordPolicy, StrengthLabel};
use dux_core::client::reload::ReloadAnswer;
use dux_core::config::{Config, DuxPaths, StartProblem, Surface};
use dux_core::config_keys::{self, GetValue, Key, SecretKind, SetPasswordError, WritePolicy};
use zeroize::Zeroizing;

/// What the live strength line measures against: the configured minimums
/// and the words that count against a password, the same ones the final
/// check uses.
pub(crate) type Meter<'a> = (&'a PasswordPolicy, &'a [&'a str]);

/// Where a secret setting's value comes from. The real one reads the
/// terminal and standard input; tests hand in canned answers.
pub(crate) trait SecretSource {
    /// Everything piped to standard input, minus one trailing line break.
    fn read_stdin(&mut self) -> Result<Password>;
    /// Ask twice on the terminal, without echo, the first time under
    /// `label` and, with a `meter`, showing the strength of the answer as it
    /// is typed. `None` when there is no terminal to ask on.
    fn prompt_twice(
        &mut self,
        label: &str,
        meter: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>>;
}

/// `dux config get <path>`: print the value config.toml holds for the
/// setting, or its default when the file leaves it out (said on stderr, so
/// stdout stays just the value for scripts).
///
/// A secret (an `env` value, or a table that holds them) is printed only with
/// `--show`; without it, `get` says the setting is set and prints nothing.
pub(crate) fn run_get(
    args: &[String],
    paths: &DuxPaths,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<()> {
    // The terminal UI's own start check joins the one list `get` reports from.
    crate::config::install_canonical_renderer();
    let show = args.iter().any(|a| a == "--show");
    let rest: Vec<&String> = args.iter().filter(|a| *a != "--show").collect();
    let [path] = rest.as_slice() else {
        bail!("usage: dux config get <setting> [--show], for example `dux config get server.port`");
    };
    let path = path.as_str();
    if path.starts_with('-') {
        bail!("{UNKNOWN_FLAG}");
    }
    let key = config_keys::lookup(path).map_err(|e| anyhow!("{e}"))?;
    // Read exactly as a start reads it: no file is the defaults, while a
    // link to a file that is gone (or a file that cannot be read) stops
    // every surface, so there is no value in use to report.
    let raw = match dux_core::config::read_config_text(&paths.config_path) {
        Ok(raw) => raw.unwrap_or_default(),
        Err(error) => bail!(
            "{error}. Neither the terminal UI nor dux server starts with it, so no value of \
             {} is in use.",
            dux_core::config::shown_path("", &key.path)
        ),
    };
    // The password itself is never stored, only its hash, so what is
    // reported is the hash, under its own name.
    let is_password = matches!(
        key.policy,
        WritePolicy::Secret(SecretKind::PasswordHash { .. })
    );
    let stored_at = match key.policy {
        WritePolicy::Secret(SecretKind::PasswordHash { stores_at }) => stores_at
            .iter()
            .map(|segment| (*segment).to_string())
            .collect(),
        _ => key.path.clone(),
    };
    // Every path printed below goes through the one formatter.
    let path = &dux_core::config::shown_path(&raw, &stored_at);
    let report = config_keys::get_report_at(&raw, &key, &paths.root)?;
    // A plaintext password written in the file is never read, and never
    // printed: it is said to be there, with where, and what to do.
    if is_password {
        for problem in dux_core::config::plaintext_password_problems(&raw) {
            writeln!(err, "({})", problem.message)?;
        }
    }
    let hide = key.is_sensitive() && !show;
    // Every surface that starts with the file uses the same value, so it is
    // said once, for the surfaces that use it; each one that will not start
    // is named on its own.
    let refuses = |surface: Surface| report.refused_by.iter().any(|(s, _)| *s == surface);
    // `[keys]` is the terminal UI's alone.
    let users = match (refuses(Surface::TerminalUi), refuses(Surface::DuxServer)) {
        _ if key.path.first().map(String::as_str) == Some("keys") => "the terminal UI",
        (false, false) => "dux",
        (true, false) => "dux server",
        (false, true) => "the terminal UI",
        (true, true) => "dux",
    };
    let corrected = !report.corrections.is_empty();
    // A table prints a name that breaks its map's rule as a marker naming
    // its line; only `--show` prints it as the file names it.
    let with_names = report.with_names.clone().filter(|_| show);
    match report.value {
        GetValue::Set(_) if hide => writeln!(
            err,
            "{path} is set; it can hold secrets, so its value is not printed. Add --show to \
             print it."
        )?,
        GetValue::Set(value) => writeln!(out, "{}", with_names.unwrap_or(value))?,
        GetValue::Default(value) if value.is_empty() => {
            writeln!(out)?;
            match key.path.as_slice() {
                [section, name, field] if section == "providers" && field == "command" => writeln!(
                    err,
                    "(config.toml lists providers.{name} without a command, so as {users} reads \
                     it the command is empty: that provider has no command and cannot start)"
                )?,
                _ => writeln!(
                    err,
                    "({path} is not in config.toml; as {users} reads the file it is empty)"
                )?,
            }
        }
        // Where the value came from is said by its correction (carried over
        // from a deprecated key, or from a retired binding), not as a default.
        GetValue::Default(value) if corrected => writeln!(out, "{value}")?,
        GetValue::Default(value) => {
            writeln!(out, "{value}")?;
            writeln!(
                err,
                "({path} is not set in config.toml; {users} uses this value)"
            )?;
        }
        GetValue::Unset if corrected => {}
        GetValue::Unset => writeln!(err, "{path} is not set")?,
        GetValue::Unknown { in_file, reason } => {
            match in_file {
                Some(_) if hide => writeln!(
                    err,
                    "{path} is set in config.toml; it can hold secrets, so its value is not \
                     printed. Add --show to print it."
                )?,
                Some(value) => writeln!(out, "{}", with_names.unwrap_or(value))?,
                None => writeln!(err, "{path} is not in config.toml")?,
            }
            writeln!(
                err,
                "(the value dux would use cannot be worked out, because {reason}; what is shown \
                 is what the file says)"
            )?;
        }
    }
    // One line per value the load changes: the setting asked for, or each
    // entry inside the table asked for.
    for correction in &report.corrections {
        let at = dux_core::config::shown_path(&raw, &correction.path);
        let reason = correction.reason.trim_end_matches('.');
        // An entry the formatter does not name has no value printed, at it
        // or below it (see `config_keys::printed_value`): only that the file
        // has one there, and what dux does with it.
        if correction.value_hidden {
            let who = match correction.surface {
                Some(Surface::TerminalUi) => "the terminal UI",
                Some(Surface::DuxServer) => "dux server",
                None => users,
            };
            match &correction.used {
                None => writeln!(
                    err,
                    "({at}: config.toml has a value here, but {who} drops that entry when it \
                     loads the file, so it is not set: {reason})"
                )?,
                Some(_) => writeln!(
                    err,
                    "({at}: config.toml has a value here; {who} uses another because {reason})"
                )?,
            }
            continue;
        }
        // A correction only one surface makes is said for that surface.
        if let Some(surface) = correction.surface {
            let name = match surface {
                Surface::TerminalUi => "the terminal UI",
                Surface::DuxServer => "dux server",
            };
            match (&correction.in_file, &correction.used) {
                (Some(in_file), Some(used)) if !hide => writeln!(
                    err,
                    "({at}: config.toml says {in_file}; {name} uses {used} because {reason})"
                )?,
                (None, Some(used)) if !hide => {
                    writeln!(err, "({at}: {name} uses {used}: {reason})")?
                }
                _ => writeln!(err, "({at}: {reason})")?,
            }
            continue;
        }
        let Some(in_file) = &correction.in_file else {
            // Left out of the file; the load writes it from a deprecated key.
            match &correction.used {
                Some(used) if !hide => writeln!(err, "({at}: {users} uses {used}, {reason})")?,
                _ => writeln!(err, "({at}: {users} uses a value {reason})")?,
            }
            continue;
        };
        let in_file = if hide { "a value" } else { in_file.as_str() };
        match &correction.used {
            Some(used) if !hide => writeln!(
                err,
                "({at}: config.toml says {in_file}; {users} uses {used} because {reason})"
            )?,
            Some(_) => writeln!(
                err,
                "({at}: config.toml says {in_file}; {users} uses another because {reason})"
            )?,
            None => writeln!(
                err,
                "({at}: config.toml says {in_file}, but {users} drops that entry when it loads \
                 the file, so it is not set: {reason})"
            )?,
        }
    }
    for (surface, why) in &report.refused_by {
        let name = match surface {
            Surface::TerminalUi => "the terminal UI",
            Surface::DuxServer => "dux server",
        };
        writeln!(
            err,
            "({name} will not start with this file, so it has no value in use for {path}: \
             {why})"
        )?;
    }
    Ok(())
}

/// `dux config set <path> [value] [--stdin]`.
pub(crate) fn run_set(
    args: &[String],
    paths: &DuxPaths,
    secrets: &mut dyn SecretSource,
    out: &mut dyn Write,
) -> Result<()> {
    // A secret setting takes nothing on the command line but itself and
    // `--stdin`: anything else might be the secret, so it is never repeated.
    if let Some(first) = args.iter().find(|arg| !arg.starts_with("--"))
        && let Ok(key) = config_keys::lookup(first)
        && matches!(key.policy, WritePolicy::Secret(_))
        && args.iter().any(|arg| arg != first && arg != "--stdin")
    {
        let what = match key.policy {
            WritePolicy::Secret(SecretKind::PasswordHash { .. }) => "the password",
            _ => "the value",
        };
        bail!(
            "unexpected argument; {what} is never given on the command line, use the prompt \
             or --stdin. Nothing was changed."
        );
    }
    let parsed = parse_set_args(args)?;
    let key = config_keys::lookup(&parsed.path).map_err(|e| anyhow!("{e}"))?;
    // A first `set` on a machine dux never ran on writes the whole commented
    // template, as dux itself would, and makes the directory for it.
    crate::config::install_canonical_renderer();
    paths.ensure_dirs()?;
    let remaining = match key.policy {
        WritePolicy::Secret(_) => set_secret(&key, parsed, paths, secrets, out)?,
        WritePolicy::Plain => {
            if parsed.stdin {
                bail!(
                    "--stdin is only for settings dux asks for, like server.auth.password and \
                     env values"
                );
            }
            let Some(value) = parsed.value else {
                bail!(
                    "{} needs a value: dux config set {} <value>",
                    parsed.path,
                    parsed.path
                );
            };
            let report = config_keys::set_plain_with(
                &paths.config_path,
                missing_file_check(paths),
                &key,
                &value,
            )?;
            writeln!(
                out,
                "{}: {} -> {} (in {})",
                shown(paths, &report.path),
                report.previous.as_deref().unwrap_or("(not set)"),
                report.now,
                paths.config_path.display()
            )?;
            // Written inside an entry or section the load already drops or
            // resets over another of its values: said, so a set is never
            // taken as in effect when it is not.
            if let Some(reason) = &report.held_back {
                writeln!(
                    out,
                    "{} is written, but it stays at its default until the problems below are \
                     fixed: {}.",
                    shown(paths, &report.path),
                    reason.trim_end_matches('.')
                )?;
            }
            (report.remaining_problems, false)
        }
    };
    let (remaining, password) = remaining;
    // What the file now is, which holds whatever is running.
    write_surface_verdicts(out, &remaining)?;
    // What happened to a running dux is said only from the outcome of
    // asking it, and only once that outcome is known.
    let wait = dux_core::client::wait::wait_timeout(None, &paths.config_path)
        .unwrap_or_else(|_| Duration::from_secs(Config::default().cli.wait_timeout_seconds));
    let answer = dux_core::client::reload::ask_to_reload(&paths.lock_path, wait);
    say_reload(&answer, !remaining.is_empty(), out)?;
    if password && let Some(sentence) = password_sentence(&answer) {
        writeln!(out, "{sentence}")?;
    }
    Ok(())
}

/// Say how the running dux answered the request to reload. A reload it
/// refused is the one answer that fails the command (exit 1): its sentence
/// says the file is saved but not in force, and why, and goes out as the error
/// so it reaches standard error.
fn say_reload(answer: &ReloadAnswer, problems_remain: bool, out: &mut dyn Write) -> Result<()> {
    let sentence = reload_sentence(answer, problems_remain);
    if matches!(answer, ReloadAnswer::Refused(_)) {
        bail!("{sentence}");
    }
    writeln!(out, "{sentence}")?;
    Ok(())
}

/// A setting's path as printed, through the one formatter, against the
/// file as it is now.
fn shown(paths: &DuxPaths, path: &[String]) -> String {
    let raw = std::fs::read_to_string(&paths.config_path).unwrap_or_default();
    dux_core::config::shown_path(&raw, path)
}

/// The problems that stop `surface` (at start, and on a reload, which
/// keeps the running settings): for `dux server`, `reload` leaves out a
/// problem its `--port`/`--bind` takes the place of, since a running one
/// was started past it.
fn stopping(problems: &[StartProblem], surface: Surface, reload: bool) -> Vec<&StartProblem> {
    problems
        .iter()
        .filter(|problem| problem.stops(surface))
        .filter(|problem| {
            !(reload && surface == Surface::DuxServer && problem.dux_server_override.is_some())
        })
        .collect()
}

/// When problems are left in the file, what each surface makes of the FILE:
/// which will not start with it and why, and which start with it. Nothing
/// here is about a running dux, which [`reload_sentence`] says from the
/// outcome of asking it. Nothing when the file has no problem.
fn write_surface_verdicts(out: &mut dyn Write, problems: &[StartProblem]) -> Result<()> {
    if problems.is_empty() {
        return Ok(());
    }
    // A problem dux server's command line gets past names exactly the flag
    // that does, from the same place `dux server` decides with.
    let list = |out: &mut dyn Write, problems: &[&StartProblem]| -> Result<()> {
        for problem in problems {
            match problem.dux_server_override {
                Some(setting) => writeln!(
                    out,
                    "  - {} (a dux server started with {} gets past this one)",
                    problem.detail.trim_end_matches('.'),
                    setting.overriding_flags()
                )?,
                None => writeln!(out, "  - {}", problem.detail)?,
            }
        }
        Ok(())
    };
    let terminal_ui = stopping(problems, Surface::TerminalUi, false);
    if terminal_ui.is_empty() {
        writeln!(out, "The terminal UI starts with this file.")?;
    } else {
        writeln!(
            out,
            "With this file, the terminal UI will not start, nor take it on a reload, until:"
        )?;
        list(out, &terminal_ui)?;
    }
    let reload = stopping(problems, Surface::DuxServer, true);
    let start = stopping(problems, Surface::DuxServer, false);
    if !reload.is_empty() {
        writeln!(
            out,
            "With this file, dux server will not start, nor take it on a reload, until:"
        )?;
        list(out, &start)?;
    } else if !start.is_empty() {
        writeln!(
            out,
            "dux server takes this file on a reload, but a new dux server will not start with it \
             until:"
        )?;
        list(out, &start)?;
    } else {
        writeln!(out, "dux server starts with this file.")?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct SetArgs {
    path: String,
    value: Option<String>,
    stdin: bool,
}

/// A flag as it may be named in an error: what follows `=` (perhaps a
/// secret typed as `--password=…`) is never repeated.
/// The refusal of a flag `dux config` does not take. It repeats nothing that
/// was typed: `--name=<value>` may carry a value, a password above all.
const UNKNOWN_FLAG: &str = "unknown flag (not repeated, in case it holds a value): `dux config \
     get` takes --show and `dux config set` takes --stdin";

/// `<path> [value] [--stdin]`. A value may start with a single dash (a
/// negative number); `--` ends the flags for one that starts with two.
fn parse_set_args(args: &[String]) -> Result<SetArgs> {
    let mut positionals: Vec<&String> = Vec::new();
    let mut stdin = false;
    let mut flags_done = false;
    for arg in args {
        if !flags_done && arg == "--" {
            flags_done = true;
        } else if !flags_done && arg == "--stdin" {
            stdin = true;
        } else if !flags_done && arg.starts_with("--") {
            bail!("{UNKNOWN_FLAG}");
        } else {
            positionals.push(arg);
        }
    }
    match positionals.as_slice() {
        [path] => Ok(SetArgs {
            path: path.to_string(),
            value: None,
            stdin,
        }),
        [path, value] => Ok(SetArgs {
            path: path.to_string(),
            value: Some(value.to_string()),
            stdin,
        }),
        [] => bail!(
            "usage: dux config set <setting> <value>, for example `dux config set server.port 4000`"
        ),
        _ => bail!(
            "dux config set takes one setting and one value; quote a value with spaces, and \
             write a list as one TOML array: '[\"a\", \"b\"]'"
        ),
    }
}

fn set_secret(
    key: &Key,
    parsed: SetArgs,
    paths: &DuxPaths,
    secrets: &mut dyn SecretSource,
    out: &mut dyn Write,
) -> Result<(Vec<StartProblem>, bool)> {
    if parsed.value.is_some() {
        // The value is deliberately not repeated: it may be the password.
        bail!(
            "{} is never given on the command line, where shell history and the process list \
             would keep it. Run `dux config set {}` to be asked for it, or pipe it in with \
             `--stdin`. Nothing was changed.",
            key.dotted(),
            key.dotted()
        );
    }
    let is_password = matches!(
        key.policy,
        WritePolicy::Secret(SecretKind::PasswordHash { .. })
    );
    // A password is checked against the policy the file sets, read key by
    // key; a policy key that does not read stops here, before asking.
    let policy = if is_password {
        config_keys::current_password_policy(&paths.config_path)?
    } else {
        dux_core::config::ServerAuthConfig::default().password_policy()
    };
    let user_inputs = user_inputs();
    let inputs: Vec<&str> = user_inputs.iter().map(String::as_str).collect();
    let password = if parsed.stdin {
        secrets.read_stdin().map_err(|error| {
            if !error.is::<StdinTooLarge>() {
                error
            } else if is_password {
                anyhow!(
                    "standard input held more than any password dux accepts; nothing was changed"
                )
            } else {
                anyhow!(
                    "standard input held an environment value larger than 1 MiB, which dux does \
                     not accept; nothing was changed"
                )
            }
        })?
    } else {
        let label = if is_password {
            "New web UI password".to_string()
        } else {
            format!("Value for {}", key.dotted())
        };
        let meter = is_password.then_some((&policy, &inputs[..]));
        let Some((first, second)) = secrets.prompt_twice(&label, meter)? else {
            bail!(
                "there is no terminal to ask for {} on; pipe it in with `--stdin`",
                key.dotted()
            );
        };
        if first.expose() != second.expose() {
            bail!("the two answers did not match. Nothing was changed.");
        }
        first
    };
    if !is_password {
        let remaining = config_keys::set_secret_text_with(
            &paths.config_path,
            missing_file_check(paths),
            key,
            &password,
        )?;
        writeln!(
            out,
            "{} updated in {} (the value is not shown).",
            shown(paths, &key.path),
            paths.config_path.display()
        )?;
        return Ok((remaining, false));
    }
    if password.expose().is_empty() {
        bail!(
            "an empty password is not a password. To remove the password, run \
             `dux config set server.auth.password_hash \"\"`. Nothing was changed."
        );
    }
    match config_keys::set_password_with(
        &paths.config_path,
        missing_file_check(paths),
        &password,
        &inputs,
    ) {
        // Stored. Whether it is in force anywhere is said after the running
        // dux was asked to reload, from what came of that.
        Ok(set) => {
            writeln!(
                out,
                "The web UI password is stored (strength: {}): its Argon2id hash is in \
                 server.auth.password_hash in {}, and the password itself is stored nowhere.",
                set.strength.label.as_str(),
                paths.config_path.display()
            )?;
            Ok((set.remaining_problems, true))
        }
        Err(SetPasswordError::BelowMinimums(check)) => {
            Err(anyhow!("{}", SetPasswordError::BelowMinimums(check)))
        }
        Err(SetPasswordError::Failed(error)) => Err(error),
        // Only the compare-and-set write can answer this, and `set` replaces
        // whatever the file holds.
        Err(error @ SetPasswordError::ChangedMeanwhile) => Err(anyhow!("{error}")),
    }
}

/// What a missing config.toml means to `set`: refused while a dux may be
/// running (checked inside the config write lock, when the file turns out to
/// be missing), because a fresh default file would drop that dux's settings,
/// the web password included, the moment it reloaded. With no dux running it
/// is the documented default, as a first start would write. A lock that
/// cannot be checked counts as a running dux.
fn missing_file_check(paths: &DuxPaths) -> config_keys::MissingConfig<'static> {
    let lock_path = paths.lock_path.clone();
    let config_path = paths.config_path.clone();
    config_keys::MissingConfig::CheckFirst(Box::new(move || {
        if dux_core::reload_signal::dux_may_be_running(&lock_path) {
            bail!(
                "{} is missing while dux may be running, and writing a fresh one here would \
                 drop the running settings, the web password included. Put the file back, or \
                 use Recover config in that dux to write its running settings to it, then run \
                 this again. Nothing was changed.",
                config_path.display()
            );
        }
        Ok(())
    }))
}

/// Words a guesser would try first for this person, which count against a
/// password built from them.
fn user_inputs() -> Vec<String> {
    dux_core::auth::guess_words()
}

/// What happens next, from how the running dux answered the request to
/// reload. With `problems_remain`, what each surface makes of the file is said
/// above it (see [`write_surface_verdicts`]), so this says only what the
/// running dux did, never that the change applies.
///
/// A refused reload is the one answer that fails the command, so the caller
/// turns it into an error; its sentence says the file is saved and not in
/// force, and why.
fn reload_sentence(answer: &ReloadAnswer, problems_remain: bool) -> String {
    let then = if problems_remain {
        " A kind that refuses this file will not start with it until the problems above are \
         fixed."
    } else {
        ""
    };
    match answer {
        ReloadAnswer::Applied(said) => format!("{said}{then}"),
        ReloadAnswer::PartlyApplied(said) => format!(
            "The change is saved and the running dux has it in force, but applying it did not \
             finish. {said}{then}"
        ),
        ReloadAnswer::Refused(said) => format!(
            "The change is saved in config.toml but is not in force: the running dux kept its \
             current settings, the old web UI password and its signed-in sessions included. \
             {}. Fix the file, then run a command that reloads again, or use Reload config \
             in dux.{then}",
            said.trim_end_matches('.')
        ),
        ReloadAnswer::NotRunning if problems_remain => "dux is not running. A terminal UI or \
                                                        dux server started now reads this \
                                                        file as said above, and a kind that \
                                                        refuses it will not start until the \
                                                        problems above are fixed."
            .to_string(),
        ReloadAnswer::NotRunning => {
            "dux is not running, so the change applies the next time it starts.".to_string()
        }
        ReloadAnswer::NotReached(why) => format!(
            "The change is saved, but the running dux was not asked to reload it: {why}. Until \
             it is reloaded (Reload config in dux) or restarted it keeps its current settings, \
             the old web UI password and its signed-in sessions included.{then}"
        ),
        ReloadAnswer::Unknown(why) => format!(
            "The change is saved, but the running dux has not said whether the reload worked: \
             {why}. Until it has, it may still be on its current settings.{then}"
        ),
    }
}

/// What a new web UI password does to a running dux, from how it answered the
/// request to reload and nothing else: a dux that has the new file in force
/// signs every browser out. `None` where the reload sentence already says it
/// all.
fn password_sentence(answer: &ReloadAnswer) -> Option<String> {
    match answer {
        ReloadAnswer::Applied(_) | ReloadAnswer::PartlyApplied(_) => Some(
            "The new password is in force: every browser signed in to it is signed out and logs \
             in with the new password."
                .to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The hidden prompt
// ---------------------------------------------------------------------------

/// The real [`SecretSource`]: standard input and the controlling terminal.
pub(crate) struct TerminalSecrets;

impl SecretSource for TerminalSecrets {
    fn read_stdin(&mut self) -> Result<Password> {
        use std::io::IsTerminal;
        refuse_terminal_stdin(std::io::stdin().is_terminal())?;
        read_secret(std::io::stdin().lock())
    }

    fn prompt_twice(
        &mut self,
        label: &str,
        meter: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Ok(None);
        }
        let first = prompt_hidden(label, meter)?;
        let second = prompt_hidden("Type it again", None)?;
        Ok(Some((first, second)))
    }
}

/// A password for `dux remote login`: piped in with `from_stdin` (refused on
/// a terminal, which would show it), else asked for once on the terminal
/// without echo. With neither a pipe asked for nor a terminal, it is refused.
pub fn read_sign_in_password(from_stdin: bool, label: &str) -> Result<Password> {
    use std::io::IsTerminal;
    if from_stdin {
        return TerminalSecrets.read_stdin();
    }
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!("there is no terminal to ask for the password on; pipe it in and add --stdin");
    }
    prompt_hidden(label, None)
}

/// `--stdin` reads a pipe. On a terminal it would read what is typed with
/// the terminal echoing it, so it is refused in favour of the hidden prompt.
fn refuse_terminal_stdin(stdin_is_terminal: bool) -> Result<()> {
    if stdin_is_terminal {
        bail!(
            "--stdin reads from a pipe, and standard input here is the terminal, which would \
             show what you type. Run the same command without --stdin to be asked for it \
             without echo."
        );
    }
    Ok(())
}

/// The largest environment value `--stdin` accepts. A password has its own,
/// smaller bound, `max_password_bytes`, which its policy check applies.
const ENV_VALUE_STDIN_LIMIT: u64 = 1024 * 1024;

/// The most read from a pipe: the largest value accepted, plus a line break.
/// More than that is refused rather than buffered.
const STDIN_LIMIT: u64 = ENV_VALUE_STDIN_LIMIT + 2;

/// Standard input held more than [`STDIN_LIMIT`]. The caller words it, since
/// only it knows whether a password or an environment value was being read.
#[derive(Debug)]
struct StdinTooLarge;

impl std::fmt::Display for StdinTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "standard input held more than 1 MiB, more than any value dux accepts; nothing was \
             changed"
        )
    }
}

impl std::error::Error for StdinTooLarge {}

/// Read a secret from `input`: everything up to end of input, minus one
/// trailing `\n` or `\r\n`, in a buffer wiped on drop.
fn read_secret(input: impl Read) -> Result<Password> {
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take(STDIN_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow!("could not read standard input: {e}"))?;
    if bytes.len() as u64 > STDIN_LIMIT {
        return Err(StdinTooLarge.into());
    }
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    Password::from_utf8(std::mem::take(&mut *bytes)).map_err(|e| anyhow!("{e}"))
}

/// What one key press does to a hidden entry.
#[derive(Debug, PartialEq, Eq)]
enum EntryStep {
    Continue,
    Done,
    Cancel,
}

/// The text typed so far into a hidden prompt, wiped on drop. Pure, so the
/// key handling is tested without a terminal.
#[derive(Default)]
struct HiddenEntry {
    text: Zeroizing<String>,
}

impl HiddenEntry {
    fn key(&mut self, key: crossterm::event::KeyEvent) -> EntryStep {
        use crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => EntryStep::Done,
            KeyCode::Esc => EntryStep::Cancel,
            KeyCode::Char('c' | 'd') if ctrl => EntryStep::Cancel,
            KeyCode::Char('u') if ctrl => {
                self.text.clear();
                EntryStep::Continue
            }
            KeyCode::Char(c) if !ctrl => {
                self.text.push(c);
                EntryStep::Continue
            }
            KeyCode::Backspace => {
                self.text.pop();
                EntryStep::Continue
            }
            _ => EntryStep::Continue,
        }
    }

    fn password(&self) -> Password {
        Password::new(self.text.as_str().to_string())
    }

    /// The prompt line: its label and, while there is text and a policy, how
    /// strong it is and whether it meets the minimums. Never the text.
    fn line(&self, label: &str, meter: Option<Meter<'_>>) -> String {
        let Some((policy, inputs)) = meter else {
            return format!("{label}: ");
        };
        if self.text.is_empty() {
            return format!("{label}: ");
        }
        let check = dux_core::auth::check_minimums(&self.password(), policy, inputs);
        let verdict = if check.passes() {
            ""
        } else if check
            .failures
            .contains(&dux_core::auth::MinimumFailure::ControlCharacter)
        {
            ", has a control character"
        } else if check
            .failures
            .iter()
            .any(|f| matches!(f, dux_core::auth::MinimumFailure::TooShort { .. }))
        {
            ", too short"
        } else if check
            .failures
            .iter()
            .any(|f| matches!(f, dux_core::auth::MinimumFailure::TooLong { .. }))
        {
            ", too long"
        } else {
            ", below the minimum"
        };
        format!(
            "{label} [{}{verdict}]: ",
            strength_meter(check.strength.label)
        )
    }
}

/// `weak -> fair -> good -> strong -> excellent`, with the reached word in
/// capitals so it reads without color.
fn strength_meter(label: StrengthLabel) -> String {
    [
        StrengthLabel::Weak,
        StrengthLabel::Fair,
        StrengthLabel::Good,
        StrengthLabel::Strong,
        StrengthLabel::Excellent,
    ]
    .iter()
    .map(|step| {
        if *step == label {
            step.as_str().to_uppercase()
        } else {
            step.as_str().to_string()
        }
    })
    .collect::<Vec<_>>()
    .join(" > ")
}

/// Ask for one hidden line on the terminal, redrawing the prompt (and its
/// strength meter when `policy` is given) after every key. The text is never
/// echoed. Ctrl-c, Ctrl-d and Esc cancel.
fn prompt_hidden(label: &str, meter: Option<Meter<'_>>) -> Result<Password> {
    use crossterm::event::{Event, KeyEventKind, read};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    struct RawGuard;
    impl Drop for RawGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }

    let mut stderr = std::io::stderr();
    let mut entry = HiddenEntry::default();
    enable_raw_mode()?;
    let _guard = RawGuard;
    let draw = |entry: &HiddenEntry, stderr: &mut std::io::Stderr| -> Result<()> {
        write!(stderr, "\r\x1b[2K{}", entry.line(label, meter))?;
        stderr.flush()?;
        Ok(())
    };
    draw(&entry, &mut stderr)?;
    loop {
        let Event::Key(key) = read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match entry.key(key) {
            EntryStep::Continue => draw(&entry, &mut stderr)?,
            EntryStep::Done => break,
            EntryStep::Cancel => {
                write!(stderr, "\r\n")?;
                bail!("cancelled; nothing was changed");
            }
        }
    }
    write!(stderr, "\r\n")?;
    Ok(entry.password())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    const COMMENTED: &str = "\
# dux configuration

[server]
# The port dux listens on.
port = 3890
";

    struct Canned {
        stdin: Option<&'static str>,
        prompts: Option<(&'static str, &'static str)>,
    }

    impl SecretSource for Canned {
        fn read_stdin(&mut self) -> Result<Password> {
            read_secret(self.stdin.expect("stdin was not expected").as_bytes())
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            Ok(self
                .prompts
                .map(|(a, b)| (Password::new(a.to_string()), Password::new(b.to_string()))))
        }
    }

    fn no_secrets() -> Canned {
        Canned {
            stdin: None,
            prompts: None,
        }
    }

    fn setup(body: Option<&str>) -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        if let Some(body) = body {
            std::fs::write(&paths.config_path, body).expect("seed");
        }
        (tmp, paths)
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn get(paths: &DuxPaths, path: &str) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_get(&args(&[path]), paths, &mut out, &mut err).expect("get");
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn set(paths: &DuxPaths, list: &[&str], secrets: &mut Canned) -> Result<String> {
        let mut out = Vec::new();
        run_set(&args(list), paths, secrets, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn get_prints_the_files_value_or_the_default() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        assert_eq!(get(&paths, "server.port"), ("3890\n".into(), String::new()));
        let (out, err) = get(&paths, "server.host");
        assert_eq!(out, "127.0.0.1\n");
        assert!(
            err.contains("not set in config.toml") && err.contains("dux uses"),
            "{err}"
        );
    }

    #[test]
    fn get_works_with_no_config_file_at_all() {
        let (_tmp, paths) = setup(None);
        assert_eq!(get(&paths, "server.port").0, "3890\n");
    }

    #[test]
    fn get_refuses_an_unknown_setting_with_a_suggestion() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let err = run_get(
            &args(&["server.prot"]),
            &paths,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .expect_err("unknown");
        assert!(
            err.to_string().contains("did you mean server.port?"),
            "{err}"
        );
    }

    #[test]
    fn set_writes_one_value_keeps_comments_and_says_what_changed() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let said = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect("set");
        assert!(said.contains("server.port: 3890 -> 4000"), "{said}");
        assert!(said.contains("not running"), "{said}");
        assert_eq!(
            std::fs::read_to_string(&paths.config_path).unwrap(),
            COMMENTED.replace("port = 3890", "port = 4000")
        );
        assert_eq!(get(&paths, "server.port").0, "4000\n");
    }

    #[test]
    fn set_refuses_unknown_keys_bad_values_and_stray_flags_without_writing() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        for (list, needle) in [
            (vec!["server.prot", "4000"], "did you mean server.port?"),
            (vec!["server.port", "lots"], "whole number"),
            (vec!["server.port"], "needs a value"),
            (vec!["server.port", "4000", "--force"], "unknown flag"),
            (vec!["server.port", "--stdin"], "--stdin is only"),
            (
                vec!["server.auth.require", "lan"],
                "network, tailnet, everywhere",
            ),
        ] {
            let err = set(&paths, &list, &mut no_secrets()).expect_err("refused");
            assert!(err.to_string().contains(needle), "{list:?}: {err}");
            assert_eq!(
                std::fs::read_to_string(&paths.config_path).unwrap(),
                COMMENTED
            );
        }
    }

    #[test]
    fn a_value_may_start_with_a_dash() {
        assert_eq!(
            parse_set_args(&args(&["ui.left_width_pct", "-1"]))
                .unwrap()
                .value
                .as_deref(),
            Some("-1")
        );
        assert_eq!(
            parse_set_args(&args(&["server.title", "--", "--loud--"]))
                .unwrap()
                .value
                .as_deref(),
            Some("--loud--")
        );
    }

    #[test]
    fn the_password_is_never_taken_from_the_command_line_or_echoed() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let err = set(
            &paths,
            &["server.auth.password", "my secret words here"],
            &mut no_secrets(),
        )
        .expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("never given on the command line"), "{text}");
        assert!(
            !text.contains("my secret words"),
            "the value is not repeated: {text}"
        );
        assert_eq!(
            std::fs::read_to_string(&paths.config_path).unwrap(),
            COMMENTED
        );
    }

    #[test]
    fn a_password_from_stdin_is_stored_as_a_hash_that_verifies() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let mut secrets = Canned {
            stdin: Some("correct horse battery staple\n"),
            prompts: None,
        };
        let said = set(&paths, &["server.auth.password", "--stdin"], &mut secrets).expect("set");
        assert!(said.contains("strength:"), "{said}");
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(after.contains("# The port dux listens on."), "{after}");
        let (hash, _) = get(&paths, "server.auth.password");
        assert_eq!(
            dux_core::auth::verify_password(
                &Password::new("correct horse battery staple".to_string()),
                hash.trim()
            ),
            Ok(true),
            "the trailing newline is not part of the password"
        );
    }

    #[test]
    fn a_weak_password_is_refused_with_its_strength_and_advice() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let mut secrets = Canned {
            stdin: Some("password1234\n"),
            prompts: None,
        };
        let err =
            set(&paths, &["server.auth.password", "--stdin"], &mut secrets).expect_err("weak");
        let text = err.to_string();
        assert!(text.contains("minimum_password_score"), "{text}");
        assert!(
            text.contains("(fair)") && text.contains("common password"),
            "{text}"
        );
        assert_eq!(
            std::fs::read_to_string(&paths.config_path).unwrap(),
            COMMENTED
        );
    }

    #[test]
    fn the_prompt_must_be_answered_the_same_way_twice() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let mut mismatch = Canned {
            stdin: None,
            prompts: Some((
                "correct horse battery staple",
                "correct horse battery stapel",
            )),
        };
        let err = set(&paths, &["server.auth.password"], &mut mismatch).expect_err("mismatch");
        assert!(err.to_string().contains("did not match"), "{err}");

        let mut matching = Canned {
            stdin: None,
            prompts: Some((
                "correct horse battery staple",
                "correct horse battery staple",
            )),
        };
        set(&paths, &["server.auth.password"], &mut matching).expect("set");
        assert!(
            get(&paths, "server.auth.password_hash")
                .0
                .starts_with("$argon2id$")
        );
    }

    #[test]
    fn with_no_terminal_and_no_stdin_flag_the_password_is_refused() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let err = set(&paths, &["server.auth.password"], &mut no_secrets()).expect_err("no tty");
        assert!(err.to_string().contains("--stdin"), "{err}");
    }

    #[test]
    fn an_empty_password_is_refused_and_says_how_to_remove_one() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let mut secrets = Canned {
            stdin: Some("\n"),
            prompts: None,
        };
        let err =
            set(&paths, &["server.auth.password", "--stdin"], &mut secrets).expect_err("empty");
        assert!(err.to_string().contains("password_hash \"\""), "{err}");
    }

    #[test]
    fn stdin_beyond_any_allowed_value_is_refused() {
        let huge = "x".repeat(STDIN_LIMIT as usize + 10);
        assert!(
            read_secret(huge.as_bytes())
                .unwrap_err()
                .is::<StdinTooLarge>()
        );
        // An environment value up to 1 MiB is read whole.
        let large = "x".repeat(ENV_VALUE_STDIN_LIMIT as usize);
        assert_eq!(
            read_secret(large.as_bytes()).unwrap().byte_len(),
            large.len()
        );
        assert_eq!(read_secret(&b"abc\r\n"[..]).unwrap().expose(), "abc");
        assert_eq!(
            read_secret(&b"two\nlines\n"[..]).unwrap().expose(),
            "two\nlines"
        );
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn the_hidden_entry_edits_and_never_shows_the_text() {
        let policy = PasswordPolicy {
            minimum_length: 12,
            minimum_score: 2,
            maximum_bytes: 1024,
        };
        let mut entry = HiddenEntry::default();
        assert_eq!(
            entry.line("New web UI password", Some((&policy, &[][..]))),
            "New web UI password: "
        );
        for c in "passwordx".chars() {
            assert_eq!(entry.key(press(KeyCode::Char(c))), EntryStep::Continue);
        }
        assert_eq!(entry.key(press(KeyCode::Backspace)), EntryStep::Continue);
        let line = entry.line("New web UI password", Some((&policy, &[][..])));
        assert!(line.contains("WEAK"), "{line}");
        assert!(line.contains("too short"), "{line}");
        assert!(
            !line.contains("password]") && !line.contains("passwordx"),
            "{line}"
        );
        assert_eq!(entry.password().expose(), "password");

        entry.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for c in "correct horse battery staple".chars() {
            entry.key(press(KeyCode::Char(c)));
        }
        let line = entry.line("New web UI password", Some((&policy, &[][..])));
        assert!(line.contains("EXCELLENT"), "{line}");
        assert!(!line.contains("too short"), "{line}");
        assert_eq!(entry.key(press(KeyCode::Enter)), EntryStep::Done);
        assert_eq!(
            entry.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            EntryStep::Cancel
        );
        assert_eq!(entry.key(press(KeyCode::Esc)), EntryStep::Cancel);
        assert_eq!(entry.line("Type it again", None), "Type it again: ");
    }

    #[test]
    fn the_meter_reads_from_weak_to_excellent() {
        assert_eq!(
            strength_meter(StrengthLabel::Good),
            "weak > fair > GOOD > strong > excellent"
        );
    }

    #[test]
    fn the_reload_sentence_names_each_outcome() {
        let applied = reload_sentence(
            &ReloadAnswer::Applied("Configuration reloaded. New settings are active now.".into()),
            false,
        );
        assert_eq!(
            applied,
            "Configuration reloaded. New settings are active now."
        );
        let refused = reload_sentence(
            &ReloadAnswer::Refused("Config reload failed: bad value for ui.theme".into()),
            false,
        );
        assert!(
            refused.contains("saved in config.toml but is not in force"),
            "{refused}"
        );
        assert!(
            refused.contains("Config reload failed: bad value for ui.theme. Fix the file"),
            "the reason is said once, with one full stop: {refused}"
        );
        assert!(reload_sentence(&ReloadAnswer::NotRunning, false).contains("next time it starts"));
        let not_reached = reload_sentence(
            &ReloadAnswer::NotReached("dux (PID 7) is running but does not answer".into()),
            false,
        );
        assert!(
            not_reached.contains("dux (PID 7) is running but does not answer"),
            "the lock's reason is said: {not_reached}"
        );
        // With problems left, no sentence promises the change applies.
        for answer in [
            ReloadAnswer::Applied("Reloaded.".into()),
            ReloadAnswer::NotRunning,
            ReloadAnswer::NotReached("why".into()),
        ] {
            let said = reload_sentence(&answer, true);
            assert!(!said.contains("next time it starts"), "{said}");
            assert!(
                said.contains("until the problems above are fixed"),
                "{said}"
            );
        }
    }

    /// Only a refused reload fails the command, and it still prints what was
    /// saved before it fails.
    #[test]
    fn a_refused_reload_fails_the_command_and_nothing_else_does() {
        for answer in [
            ReloadAnswer::Applied("ok".into()),
            ReloadAnswer::PartlyApplied("one step failed".into()),
            ReloadAnswer::NotRunning,
            ReloadAnswer::NotReached("why".into()),
            ReloadAnswer::Unknown("op-1".into()),
        ] {
            let mut out = Vec::new();
            say_reload(&answer, false, &mut out).expect("not a failure");
            assert!(!out.is_empty(), "{answer:?} says something");
        }
        let mut out = Vec::new();
        let error = say_reload(&ReloadAnswer::Refused("no".into()), false, &mut out)
            .expect_err("refused fails");
        assert!(
            error.to_string().contains("not in force"),
            "the failure carries the sentence: {error}"
        );
    }

    #[test]
    fn set_can_repair_the_auth_section_dux_refuses_to_start_with() {
        let (_tmp, paths) = setup(Some("[server.auth]\nrequire = \"lan\"\n"));
        set(
            &paths,
            &["server.auth.require", "network"],
            &mut no_secrets(),
        )
        .expect("repair");
        assert!(
            dux_core::config::auth_section_of(
                &std::fs::read_to_string(&paths.config_path).unwrap()
            )
            .is_ok()
        );
    }
    /// The live meter scores with the same words the final check uses, so
    /// it cannot promise a strength the set then refuses.
    #[test]
    fn the_live_meter_counts_the_same_user_inputs_as_the_check() {
        let policy = PasswordPolicy {
            minimum_length: 1,
            minimum_score: 0,
            maximum_bytes: 1024,
        };
        let mut entry = HiddenEntry::default();
        for c in "duxdux2026dux".chars() {
            entry.key(press(KeyCode::Char(c)));
        }
        let inputs = ["dux", "someone"];
        let expected = dux_core::auth::check_minimums(&entry.password(), &policy, &inputs)
            .strength
            .label;
        let line = entry.line("p", Some((&policy, &inputs[..])));
        assert!(line.contains(&expected.as_str().to_uppercase()), "{line}");
    }

    #[test]
    fn stdin_from_a_terminal_is_refused_with_the_prompt_as_the_way() {
        let err = refuse_terminal_stdin(true).expect_err("a terminal would echo");
        assert!(err.to_string().contains("without --stdin"), "{err}");
        assert!(refuse_terminal_stdin(false).is_ok());
    }

    #[test]
    fn an_env_value_is_never_a_command_line_argument_and_never_printed_by_set() {
        let (_tmp, paths) = setup(Some(COMMENTED));
        let err = set(
            &paths,
            &["env.GITHUB_TOKEN", "ghp_secret"],
            &mut no_secrets(),
        )
        .expect_err("refused");
        assert!(!err.to_string().contains("ghp_secret"), "{err}");
        let mut piped = Canned {
            stdin: Some("ghp_secret\n"),
            prompts: None,
        };
        let said = set(&paths, &["env.GITHUB_TOKEN", "--stdin"], &mut piped).expect("set");
        assert!(said.contains("env.GITHUB_TOKEN updated"), "{said}");
        assert!(!said.contains("ghp_secret"), "{said}");
        assert!(
            std::fs::read_to_string(&paths.config_path)
                .unwrap()
                .contains("GITHUB_TOKEN = \"ghp_secret\"")
        );
    }

    #[test]
    fn get_prints_a_secret_only_with_show() {
        let (_tmp, paths) = setup(Some("[env]\nGITHUB_TOKEN = \"ghp_secret\"\n"));
        let (out, err) = get(&paths, "env.GITHUB_TOKEN");
        assert_eq!(out, "", "nothing on stdout");
        assert!(err.contains("--show"), "{err}");
        let (out, _) = get(&paths, "env");
        assert!(!out.contains("ghp_secret"), "{out}");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_get(
            &args(&["env.GITHUB_TOKEN", "--show"]),
            &paths,
            &mut out,
            &mut err,
        )
        .expect("get --show");
        assert_eq!(String::from_utf8(out).unwrap(), "ghp_secret\n");
    }

    /// `dux config set` on a machine where dux never ran writes the whole
    /// documented config, directory included, not a bare one-key file.
    #[test]
    fn set_with_no_config_writes_the_documented_file() {
        let (tmp, mut paths) = setup(None);
        let root = tmp.path().join("not-yet");
        paths.config_path = root.join("config.toml");
        paths.lock_path = root.join("dux.lock");
        paths.root = root;
        set(&paths, &["server.port", "4000"], &mut no_secrets()).expect("set");
        let written = std::fs::read_to_string(&paths.config_path).expect("created");
        assert!(written.contains("# dux configuration"), "{written}");
        assert!(written.contains("port = 4000"), "{written}");
    }

    /// A lock that cannot even be checked counts as a running dux: `set`
    /// refuses a missing file rather than guess.
    #[test]
    fn set_refuses_a_missing_config_when_the_lock_cannot_be_checked() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, paths) = setup(None);
        std::fs::write(&paths.lock_path, "").expect("lock file");
        std::fs::set_permissions(&paths.lock_path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = set(&paths, &["server.port", "4000"], &mut no_secrets());
        std::fs::set_permissions(&paths.lock_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = result.expect_err("refused");
        assert!(err.to_string().contains("Recover config"), "{err}");
        assert!(!paths.config_path.exists(), "nothing was written");
    }

    /// config.toml deleted while a dux runs: `set` must not write a fresh
    /// default (password-less) file and tell that dux to reload it. With the
    /// file there, a dux that holds the lock but does not answer on a control
    /// socket is named as not asked, and the change is still saved.
    #[test]
    fn set_refuses_a_missing_config_while_a_dux_runs_and_says_when_it_cannot_ask_it() {
        let (_tmp, paths) = setup(None);
        let _running =
            dux_core::lockfile::SingleInstanceLock::acquire(&paths.lock_path).expect("lock");
        let err = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("Recover config"), "{text}");
        assert!(!paths.config_path.exists(), "nothing was written");

        std::fs::write(&paths.config_path, "[server]\nport = 3890\n").expect("seed");
        let said = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect("saved");
        assert!(
            said.contains("is running but does not answer on its control socket; restart it"),
            "{said}"
        );
        assert!(
            std::fs::read_to_string(&paths.config_path)
                .unwrap()
                .contains("port = 4000"),
            "the change is saved though dux could not be asked"
        );
    }

    /// A password stored beside a problem the file already had says dux will
    /// not start with the file (and a running dux keeps its settings) until
    /// the named problems are fixed, never that the password is in force or
    /// that browsers were signed out.
    #[test]
    fn a_password_set_beside_a_problem_says_dux_will_not_start() {
        let (_tmp, paths) = setup(Some("[server.auth]\nrequire = \"lan\"\n"));
        let mut secrets = Canned {
            stdin: Some("correct horse battery staple\n"),
            prompts: None,
        };
        let said = set(&paths, &["server.auth.password", "--stdin"], &mut secrets)
            .expect("the set itself adds no problem");
        assert!(said.contains("will not start"), "{said}");
        assert!(said.contains("require"), "{said}");
        assert!(!said.contains("signed out"), "{said}");
        assert!(!said.contains("is set"), "{said}");
        assert!(!said.contains("applies the next time it starts"), "{said}");
        let written = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(written.contains("$argon2id$"), "the password is stored");
    }

    /// An env value stored beside a problem lists it the same way.
    #[test]
    fn an_env_value_set_beside_a_problem_lists_it() {
        let (_tmp, paths) = setup(Some("[server.auth]\nrequire = \"lan\"\n"));
        let mut secrets = Canned {
            stdin: Some("abc\n"),
            prompts: None,
        };
        let said = set(&paths, &["env.TOKEN", "--stdin"], &mut secrets).expect("set");
        assert!(said.contains("will not start"), "{said}");
        assert!(said.contains("require"), "{said}");
    }

    /// What `set` writes is a file dux starts with: a value the start
    /// refuses (a hostname as server.host, an env name outside the rule) is
    /// refused by the set, and nothing is written.
    #[test]
    fn a_set_never_writes_a_file_dux_refuses_to_start_with() {
        for (list, stdin) in [
            (vec!["server.host", "localhost"], None),
            (vec!["env.MY-TOKEN", "--stdin"], Some("abc\n")),
        ] {
            let (_tmp, paths) = setup(Some("[server]\nport = 3890\n"));
            let mut secrets = Canned {
                stdin,
                prompts: None,
            };
            set(&paths, &list, &mut secrets).expect_err("refused");
            assert_eq!(
                std::fs::read_to_string(&paths.config_path).unwrap(),
                "[server]\nport = 3890\n"
            );
            crate::config::ensure_config(&paths).expect("dux still starts");
        }
    }

    /// `get` on a provider the file lists without a command says the command
    /// is empty as dux reads the file, and that the provider has none.
    #[test]
    fn get_says_a_listed_provider_without_a_command_has_none() {
        let (_tmp, paths) = setup(Some("[providers.claude]\nargs = [\"--verbose\"]\n"));
        let (out, err) = get(&paths, "providers.claude.command");
        assert_eq!(out, "\n");
        assert!(err.contains("no command"), "{err}");
    }

    /// `get` on a file dux cannot load prints the file's own value and says
    /// the value dux would use cannot be worked out, and why.
    #[test]
    fn get_on_a_file_dux_cannot_load_names_the_problem() {
        let (_tmp, paths) = setup(Some(
            "[ui]\nleft_width_pct = 25\n\n[server.auth]\nrequire = \"lan\"\n",
        ));
        let (out, err) = get(&paths, "ui.left_width_pct");
        assert_eq!(out, "25\n");
        assert!(err.contains("cannot"), "{err}");
        assert!(err.contains("require"), "{err}");
    }

    /// A setting typed as `key=value` (other tools' habit) is refused naming
    /// only the part before `=`, so a password typed that way is never
    /// printed back; an unknown `--flag=value` is named without its value too.
    #[test]
    fn a_value_typed_after_an_equals_sign_is_never_repeated() {
        let (_tmp, paths) = setup(Some("[server]\nport = 3890\n"));
        let error = set(
            &paths,
            &["server.auth.password=Tr0ub4dor&3xyz"],
            &mut no_secrets(),
        )
        .expect_err("refused");
        let message = format!("{error:#}");
        assert!(!message.contains("Tr0ub4dor"), "{message}");
        assert!(message.contains("server.auth.password"), "{message}");
        let error = set(
            &paths,
            &["server.port", "--secret=hunter2"],
            &mut no_secrets(),
        )
        .expect_err("refused");
        assert!(!format!("{error:#}").contains("hunter2"), "{error:#}");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let error = run_get(
            &args(&["server.auth.password=hunter2"]),
            &paths,
            &mut out,
            &mut err,
        )
        .expect_err("refused");
        assert!(!format!("{error:#}").contains("hunter2"), "{error:#}");
    }

    /// A file the terminal UI refuses to start with (a wrong-typed setting)
    /// is never reported by `set` as fine: the problem is listed, and the
    /// closing line does not say the change applies at the next start.
    #[test]
    fn a_set_lists_a_problem_that_stops_the_terminal_ui_starting() {
        let (_tmp, paths) = setup(Some("[ui]\nleft_width_pct = \"wide\"\n"));
        let said = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect("set");
        crate::config::ensure_config(&paths).expect_err("the terminal UI refuses the file");
        assert!(said.contains("the terminal UI will not start"), "{said}");
        assert!(!said.contains("next time it starts"), "{said}");
        // `get` answers for dux server, which reads the wrong-typed value as
        // its default, and names the terminal UI as refusing the file.
        let (out, err) = get(&paths, "ui.left_width_pct");
        assert_eq!(out, "20\n");
        assert!(
            err.contains("config.toml says wide; dux server uses 20"),
            "{err}"
        );
        assert!(
            err.contains("the terminal UI will not start with this file"),
            "{err}"
        );
    }

    const STRONG_PASSWORD: &str = "violet-quarry-71-snowmelt-bracket\n";

    /// `[keys]` the terminal UI does not accept stop its start and its
    /// reloads, so a set over such a file lists the problem and promises no
    /// start, and a password set says a running terminal UI keeps the old
    /// one (while `dux server`, which takes the file, puts it in force).
    #[test]
    fn a_set_over_keys_the_terminal_ui_refuses_promises_no_start() {
        let (_tmp, paths) = setup(Some("[keys]\nnot_a_real_action = [\"x\"]\n"));
        let said = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect("set");
        crate::config::ensure_config(&paths).expect_err("the terminal UI refuses [keys]");
        assert!(!said.contains("applies the next time it starts"), "{said}");
        assert!(said.contains("the terminal UI will not start"), "{said}");
        let mut secrets = Canned {
            stdin: Some(STRONG_PASSWORD),
            prompts: None,
        };
        let said = set(&paths, &["server.auth.password", "--stdin"], &mut secrets).expect("set");
        assert!(
            !said.contains("Every browser signed in to dux is signed out"),
            "{said}"
        );
        // No dux runs here, so nothing is said to be in force or kept by
        // one: only what the file does to each surface.
        assert!(!said.contains("signed out"), "{said}");
        assert!(said.contains("the terminal UI will not start"), "{said}");
        assert!(said.contains("dux server starts with this file"), "{said}");
        assert!(said.contains("dux is not running"), "{said}");
    }

    /// A password reaches browsers only through a dux that has the new file in
    /// force, and only the answer of that dux says so: nothing is claimed when
    /// no dux ran, could not be asked, or refused the file.
    #[test]
    fn the_password_sentence_follows_the_reload_answer() {
        for answer in [
            ReloadAnswer::Applied("Reloaded.".into()),
            ReloadAnswer::PartlyApplied("One step failed.".into()),
        ] {
            let said = password_sentence(&answer).expect("in force");
            assert!(said.contains("is signed out"), "{said}");
        }
        for answer in [
            ReloadAnswer::NotRunning,
            ReloadAnswer::NotReached("why".into()),
            ReloadAnswer::Unknown("op-1".into()),
            ReloadAnswer::Refused("no".into()),
        ] {
            assert_eq!(password_sentence(&answer), None, "{answer:?}");
            assert!(
                !reload_sentence(&answer, false).contains("signed out"),
                "{answer:?}"
            );
        }
    }

    /// `get` shows the value dux uses after the load's corrections, and
    /// what the file says beside it with why; a retired provider's stock
    /// block that the load drops is said to be dropped.
    #[test]
    fn get_shows_what_dux_uses_and_what_the_file_says_when_they_differ() {
        let (_tmp, paths) = setup(Some("[ui]\nterminal_font_size = 500\n"));
        let (out, err) = get(&paths, "ui.terminal_font_size");
        assert_eq!(out, "14\n");
        assert!(
            err.contains("config.toml says 500; dux uses 14 because"),
            "{err}"
        );
        let (_tmp, paths) = setup(Some(
            "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\n\
             resume_wait_timeout_ms = 0\ninstall_hint = \"brew install gemini-cli\"\n",
        ));
        let (out, err) = get(&paths, "providers.gemini.command");
        assert_eq!(out, "");
        assert!(err.contains("config.toml says gemini"), "{err}");
        assert!(err.contains("drops that entry"), "{err}");
    }

    /// Duplicate project ids stop the terminal UI and `dux server` alike, so
    /// a password set over them does not claim to be in force.
    #[test]
    fn a_password_set_over_duplicate_project_ids_is_not_in_force() {
        let (_tmp, paths) = setup(Some(
            "[[projects]]\nid = \"same\"\npath = \"/tmp/review19-a\"\n\n\
             [[projects]]\nid = \"same\"\npath = \"/tmp/review19-b\"\n",
        ));
        let mut secrets = Canned {
            stdin: Some(STRONG_PASSWORD),
            prompts: None,
        };
        let said = set(&paths, &["server.auth.password", "--stdin"], &mut secrets).expect("set");
        let config = dux_core::config::load_config(&paths).expect("loads");
        dux_core::config_sync::validate_project_records("config.toml", &config.projects)
            .expect_err("dux refuses duplicate project ids at start");
        assert!(
            !said.contains("Every browser signed in to dux is signed out"),
            "{said}"
        );
        assert!(said.contains("dux server"), "{said}");
    }
}

#[cfg(test)]
mod set_speaks_per_surface_tests {
    use super::*;

    struct NoSecrets;
    impl SecretSource for NoSecrets {
        fn read_stdin(&mut self) -> Result<Password> {
            bail!("no stdin")
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            Ok(None)
        }
    }

    #[test]
    fn set_never_says_the_terminal_ui_will_not_start_with_a_file_it_starts_with() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        // Port 0 with no background server: only `dux server` refuses it, and
        // its --port overrides it.
        std::fs::write(&paths.config_path, "[server]\nport = 0\n").unwrap();
        let mut out = Vec::new();
        run_set(
            &["ui.left_width_pct".to_string(), "30".to_string()],
            &paths,
            &mut NoSecrets,
            &mut out,
        )
        .unwrap();
        let said = String::from_utf8(out).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        assert_eq!(
            dux_core::config::start_refusal(&after, dux_core::config::Surface::TerminalUi),
            None,
            "precondition: the terminal UI starts with this file and applies the change"
        );
        assert!(
            !said.contains("dux will not start with it")
                && !said.contains("it will not start with this file"),
            "set claims dux will not start although `dux` (the terminal UI) does:\n{said}"
        );
    }

    struct Piped(&'static str);
    impl SecretSource for Piped {
        fn read_stdin(&mut self) -> Result<Password> {
            Ok(Password::new(self.0.to_string()))
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            Ok(None)
        }
    }

    #[test]
    fn a_password_dux_server_will_apply_is_not_called_not_in_force() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        // An environment variable the terminal UI refuses; dux server does not.
        std::fs::write(&paths.config_path, "[env]\nFOO = \"${\"\n").unwrap();
        let mut out = Vec::new();
        run_set(
            &["server.auth.password".to_string(), "--stdin".to_string()],
            &paths,
            &mut Piped("correct horse battery staple zebra"),
            &mut out,
        )
        .unwrap();
        let said = String::from_utf8(out).unwrap();
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        assert_eq!(
            dux_core::config::start_refusal(&after, dux_core::config::Surface::DuxServer),
            None,
            "precondition: dux server starts with (and reloads) this file, password included"
        );
        assert!(
            !said.contains("not in force yet"),
            "set says the password is not in force, but a running dux server applies it:\n{said}"
        );
    }
}

#[cfg(test)]
mod names_fields_and_per_surface_get_tests {
    use super::*;

    struct Piped(&'static str);
    impl SecretSource for Piped {
        fn read_stdin(&mut self) -> Result<Password> {
            Ok(Password::new(self.0.to_string()))
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            Ok(None)
        }
    }

    fn paths_with(body: &str) -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        std::fs::write(&paths.config_path, body).unwrap();
        (tmp, paths)
    }

    fn assert_set_adds_no_start_problem(list: &[&str], secrets: &mut dyn SecretSource) {
        let before = "[server]\nport = 3890\n";
        let (_tmp, paths) = paths_with(before);
        assert_eq!(
            dux_core::config::start_refusal(before, dux_core::config::Surface::TerminalUi),
            None
        );
        let args: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let result = run_set(&args, &paths, secrets, &mut out);
        let after = std::fs::read_to_string(&paths.config_path).unwrap();
        if result.is_ok() {
            for surface in [
                dux_core::config::Surface::TerminalUi,
                dux_core::config::Surface::DuxServer,
            ] {
                assert_eq!(
                    dux_core::config::start_refusal(&after, surface),
                    None,
                    "`dux config set {list:?}` succeeded on a file every surface started with, \
                     and now {surface:?} refuses it:\n{after}"
                );
            }
            crate::config::ensure_config(&paths).expect("the terminal UI still starts");
        }
    }

    /// A provider named `password_hash` is an ordinary provider, but the set
    /// writes it and then neither surface will start.
    #[test]
    fn setting_a_provider_named_password_hash_never_writes_a_file_dux_refuses() {
        assert_set_adds_no_start_problem(
            &["providers.password_hash.command", "mytool"],
            &mut Piped(""),
        );
    }

    /// Same for an environment variable named `password_hash`, a valid name
    /// by the env rule dux starts with.
    #[test]
    fn setting_an_env_variable_named_password_hash_never_writes_a_file_dux_refuses() {
        assert_set_adds_no_start_problem(&["env.password_hash", "--stdin"], &mut Piped("x"));
    }

    /// A wrong-typed `args` already in a provider blocks a set of another
    /// key of that provider, although the set adds nothing.
    #[test]
    fn a_problem_already_in_a_providers_args_never_blocks_a_set_of_its_other_keys() {
        let body = "[providers.mytool]\ncommand = \"mytool\"\nargs = \"oops\"\n";
        let (_tmp, paths) = paths_with(body);
        let mut out = Vec::new();
        let result = run_set(
            &[
                "providers.mytool.install_hint".to_string(),
                "brew install mytool".to_string(),
            ],
            &paths,
            &mut Piped(""),
            &mut out,
        );
        assert!(
            result.is_ok(),
            "a set of providers.mytool.install_hint was refused over a problem the file \
             already had in providers.mytool.args: {:#}",
            result.unwrap_err()
        );
    }

    /// With a problem only the terminal UI refuses (an env value), dux server
    /// starts and uses the file's port, but get says that value cannot be
    /// worked out.
    #[test]
    fn get_reports_the_value_dux_server_uses_beside_a_terminal_ui_only_problem() {
        let (_tmp, paths) = paths_with("[env]\nFOO = \"${\"\n\n[server]\nport = 4000\n");
        let raw = std::fs::read_to_string(&paths.config_path).unwrap();
        assert_eq!(
            dux_core::config::start_refusal(&raw, dux_core::config::Surface::DuxServer),
            None,
            "precondition: dux server starts with this file"
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_get(&["server.port".to_string()], &paths, &mut out, &mut err).unwrap();
        let err = String::from_utf8(err).unwrap();
        assert!(
            !err.contains("cannot be worked out"),
            "dux server runs with port 4000, but get says:\n{err}"
        );
    }

    /// `get ui` on a file whose ui.terminal_font_size dux corrects at load
    /// prints the file's 999 as the value, and says nothing of the default
    /// dux uses in its place.
    #[test]
    fn get_of_a_table_says_when_dux_uses_a_corrected_value_inside_it() {
        let (_tmp, paths) = paths_with("[ui]\nterminal_font_size = 999\n");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_get(
            &["ui.terminal_font_size".to_string()],
            &paths,
            &mut out,
            &mut err,
        )
        .unwrap();
        let single = String::from_utf8(out).unwrap();
        assert!(
            !single.contains("999"),
            "precondition: the key itself reports the correction"
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_get(&["ui".to_string()], &paths, &mut out, &mut err).unwrap();
        let (out, err) = (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        );
        assert!(
            !out.contains("terminal_font_size = 999") || err.contains("terminal_font_size"),
            "get ui reports a font size of 999, which dux does not use, and says nothing:\n{out}{err}"
        );
    }
}

#[cfg(test)]
mod overridable_problems_name_their_flag_tests {
    use super::*;

    struct NoSecrets;
    impl SecretSource for NoSecrets {
        fn read_stdin(&mut self) -> Result<Password> {
            unreachable!()
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            unreachable!()
        }
    }

    /// With a `[server] host` that is not an IP already in the file, a set of
    /// an unrelated key says a dux server started "without --port or --bind"
    /// will not start, which tells the user `--port` is enough. It is not:
    /// only `--bind` replaces the host, and `dux server --port N` refuses the
    /// same file.
    #[test]
    fn set_says_port_alone_lets_dux_server_start_past_a_bad_host() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        std::fs::write(&paths.config_path, "[server]\nhost = \"localhost\"\n").unwrap();
        let args: Vec<String> = ["ui.left_width_pct", "30"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut out = Vec::new();
        run_set(&args, &paths, &mut NoSecrets, &mut out).expect("set of an unrelated key");
        let said = String::from_utf8(out).unwrap();
        let config = dux_core::config::load_config_file(&paths.config_path).unwrap();
        let with_port_only = dux_core::config::resolve_server_plan(
            &config.server,
            &dux_core::config::ServerCliOverrides {
                bind: None,
                port: Some(4000),
                no_tailscale: true,
            },
            None,
        );
        assert!(
            with_port_only.is_err(),
            "precondition: dux server --port refuses this host"
        );
        assert!(
            !said.contains("started without --port or --bind will not start"),
            "set tells the user --port gets dux server past the host, which it does not:\n{said}"
        );
    }

    /// The bad host's line names `--bind` as the one flag that gets dux
    /// server past it.
    #[test]
    fn a_bad_host_names_bind_as_the_flag_that_gets_past_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
            socket_path: tmp.path().join("dux.sock"),
        };
        std::fs::write(&paths.config_path, "[server]\nhost = \"localhost\"\n").unwrap();
        let mut out = Vec::new();
        run_set(
            &["ui.left_width_pct".to_string(), "30".to_string()],
            &paths,
            &mut NoSecrets,
            &mut out,
        )
        .expect("set");
        let said = String::from_utf8(out).unwrap();
        assert!(
            said.contains("(a dux server started with --bind gets past this one)"),
            "{said}"
        );
        assert!(!said.contains("--port or --bind gets past"), "{said}");
    }
}

#[cfg(test)]
mod printing_signals_and_carried_values_tests;

#[cfg(test)]
mod rule_breaking_names_in_tables_tests;

#[cfg(test)]
mod dotted_names_migrations_and_providers_tests;

#[cfg(test)]
mod sets_over_the_start_corpus_tests;

#[cfg(test)]
mod get_names_every_setting_in_use_tests;

#[cfg(test)]
mod get_keys_and_effective_tables_tests;

#[cfg(test)]
mod effective_tables_and_secret_arguments_tests;

#[cfg(test)]
mod get_sources_and_refusing_surfaces_tests;

#[cfg(test)]
mod source_markers_and_path_echo_tests;

#[cfg(test)]
mod get_never_prints_unknown_names_tests;

#[cfg(test)]
mod names_never_printed_property_tests;

#[cfg(test)]
mod effective_values_and_array_names_tests;

#[cfg(test)]
mod inline_keys_set_tests;

#[cfg(test)]
mod names_under_unknown_keys_tests;

#[cfg(test)]
mod hidden_values_tests;

#[cfg(test)]
mod password_control_characters_tests;

#[cfg(test)]
mod restore_report_and_unknown_path_tests;
#[cfg(test)]
mod stdin_limits_tests;

#[cfg(test)]
mod key_binding_values_tests;

#[cfg(test)]
mod conflicting_binding_tests;

#[cfg(test)]
mod set_get_policy_projects_paths_tests;

#[cfg(test)]
mod plaintext_password_and_theme_tests;

#[cfg(test)]
mod plaintext_shape_get_tests;

#[cfg(test)]
mod appended_password_tests;
