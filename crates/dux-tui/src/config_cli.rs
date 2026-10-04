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
//! own lock, and it then signals the dux holding `dux.lock` with SIGUSR1
//! (see [`dux_core::reload_signal`]).

use std::io::{Read, Write};

use anyhow::{Result, anyhow, bail};
use dux_core::auth::{Password, PasswordPolicy, StrengthLabel};
use dux_core::config::{DuxPaths, StartProblem, Surface};
use dux_core::config_keys::{self, GetValue, Key, SecretKind, SetPasswordError, WritePolicy};
use dux_core::reload_signal::{NoReloadHandler, SignalOutcome};
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
        bail!("unknown flag: {}", flag_name(path));
    }
    let key = config_keys::lookup(path).map_err(|e| anyhow!("{e}"))?;
    let raw = match std::fs::read_to_string(&paths.config_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => bail!("could not read {}: {error}", paths.config_path.display()),
    };
    let report = config_keys::get_report(&raw, &key)?;
    let hide = key.is_sensitive() && !show;
    // Every surface that starts with the file uses the same value, so it is
    // said once, for the surfaces that use it; each one that will not start
    // is named on its own.
    let refuses = |surface: Surface| report.refused_by.iter().any(|(s, _)| *s == surface);
    let users = match (refuses(Surface::TerminalUi), refuses(Surface::DuxServer)) {
        (false, false) => "dux",
        (true, false) => "dux server",
        (false, true) => "the terminal UI",
        (true, true) => "dux",
    };
    let corrected = !report.corrections.is_empty();
    match report.value {
        GetValue::Set(_) if hide => writeln!(
            err,
            "{path} is set; it can hold secrets, so its value is not printed. Add --show to \
             print it."
        )?,
        GetValue::Set(value) => writeln!(out, "{value}")?,
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
                Some(value) => writeln!(out, "{value}")?,
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
        let at = &correction.path;
        let reason = correction.reason.trim_end_matches('.');
        let in_file = if hide {
            "a value"
        } else {
            correction.in_file.as_str()
        };
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
                report.path,
                report.previous.as_deref().unwrap_or("(not set)"),
                report.now,
                paths.config_path.display()
            )?;
            write_surface_verdicts(out, &report.remaining_problems)?;
            report.remaining_problems
        }
    };
    let outcome = dux_core::reload_signal::signal_running_dux(&paths.lock_path);
    writeln!(out, "{}", reload_sentence(&outcome, !remaining.is_empty()))?;
    Ok(())
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

/// When problems are left in the file, what each surface makes of it, said
/// only for the surfaces it is true of: which will not start (and, when
/// running, keep their settings) and why, and which apply it. Which kind of
/// dux holds the lock cannot be told from outside, so both are said, each
/// for itself. Nothing when the file has no problem.
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
        writeln!(
            out,
            "The terminal UI applies it (a running terminal UI reloads it now)."
        )?;
    } else {
        writeln!(
            out,
            "With this file, the terminal UI will not start (and a running terminal UI keeps its \
             settings) until:"
        )?;
        list(out, &terminal_ui)?;
    }
    let reload = stopping(problems, Surface::DuxServer, true);
    let start = stopping(problems, Surface::DuxServer, false);
    if !reload.is_empty() {
        writeln!(
            out,
            "With this file, dux server will not start (and a running dux server keeps its \
             settings) until:"
        )?;
        list(out, &start)?;
    } else if !start.is_empty() {
        writeln!(
            out,
            "dux server applies it (a running dux server reloads it now), but a new dux server \
             will not start with this file until:"
        )?;
        list(out, &start)?;
    } else {
        writeln!(
            out,
            "dux server applies it (a running dux server reloads it now)."
        )?;
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
fn flag_name(arg: &str) -> &str {
    arg.split_once('=').map_or(arg, |(name, _)| name)
}

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
            bail!("unknown flag: {}", flag_name(arg));
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
) -> Result<Vec<StartProblem>> {
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
        secrets.read_stdin()?
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
            key.dotted(),
            paths.config_path.display()
        )?;
        write_surface_verdicts(out, &remaining)?;
        return Ok(remaining);
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
        Ok(set) if !set.remaining_problems.is_empty() => {
            // Stored; in force only on a surface that takes the file. Each
            // surface is said for itself, since which one is running cannot
            // be told from outside.
            writeln!(
                out,
                "The web UI password is stored (strength: {}): its Argon2id hash is in \
                 server.auth.password_hash in {}, and the password itself is stored nowhere.",
                set.strength.label.as_str(),
                paths.config_path.display()
            )?;
            let problems = &set.remaining_problems;
            if stopping(problems, Surface::DuxServer, true).is_empty() {
                writeln!(
                    out,
                    "A running dux server puts it in force now: every browser signed in to it is \
                     signed out and logs in with the new password."
                )?;
            } else {
                writeln!(
                    out,
                    "A running dux server keeps the old password until the problems below are \
                     fixed."
                )?;
            }
            if stopping(problems, Surface::TerminalUi, true).is_empty() {
                writeln!(
                    out,
                    "A running terminal UI puts it in force for its background server now: every \
                     browser signed in to that is signed out and logs in with the new password."
                )?;
            } else {
                writeln!(
                    out,
                    "A running terminal UI keeps the old password until the problems below are \
                     fixed."
                )?;
            }
            write_surface_verdicts(out, problems)?;
            Ok(set.remaining_problems)
        }
        Ok(set) => {
            writeln!(
                out,
                "The web UI password is set (strength: {}). Its Argon2id hash is in \
                 server.auth.password_hash in {}; the password itself is stored nowhere.",
                set.strength.label.as_str(),
                paths.config_path.display()
            )?;
            writeln!(
                out,
                "Every browser signed in to dux is signed out and logs in with the new password."
            )?;
            Ok(Vec::new())
        }
        Err(SetPasswordError::BelowMinimums(check)) => {
            Err(anyhow!("{}", SetPasswordError::BelowMinimums(check)))
        }
        Err(SetPasswordError::Failed(error)) => Err(error),
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
    let mut inputs = vec!["dux".to_string()];
    if let Ok(user) = std::env::var("USER")
        && !user.is_empty()
    {
        inputs.push(user);
    }
    inputs
}

/// What happens next. With `problems_remain`, what each surface makes of
/// the file is said above it (see [`write_surface_verdicts`]), so this says
/// only what was done about the running dux, never that the change applies.
fn reload_sentence(outcome: &SignalOutcome, problems_remain: bool) -> String {
    let then = if problems_remain {
        " A kind that refuses this file will not start with it until the problems above are \
         fixed."
    } else {
        ""
    };
    // `kill -USR1` is offered only for a dux that has said it handles the
    // signal: sent to one that does not, it stops that dux.
    let by_hand = |pid: &Option<u32>| match pid {
        Some(pid) => format!("Run Reload config in dux, or `kill -USR1 {pid}`."),
        None => "Run Reload config in dux.".to_string(),
    };
    match outcome {
        SignalOutcome::Sent { pid } if problems_remain => format!(
            "Asked the running dux (PID {pid}) to reload its config: if it is a kind that \
             refuses this file, it keeps its current settings until the problems above are \
             fixed. It says whether the reload worked in its status line, in the web UI's \
             notifications and in dux.log."
        ),
        SignalOutcome::Sent { pid } => format!(
            "Asked the running dux (PID {pid}) to reload its config. It says whether the \
             reload worked in its status line, in the web UI's notifications and in dux.log."
        ),
        SignalOutcome::NotRunning if problems_remain => "dux is not running. A terminal UI or \
                                                         dux server started now reads this \
                                                         file as said above, and a kind that \
                                                         refuses it will not start until the \
                                                         problems above are fixed."
            .to_string(),
        SignalOutcome::NotRunning => {
            "dux is not running, so the change applies the next time it starts.".to_string()
        }
        SignalOutcome::NotSignalled {
            pid,
            why: NoReloadHandler::OlderVersion,
        } => format!(
            "The running dux (PID {pid}) is an older version that cannot reload its config while \
             running; the change applies when you restart it.{then}"
        ),
        SignalOutcome::NotSignalled {
            pid,
            why: NoReloadHandler::NotCaught,
        } => format!(
            "The running dux (PID {pid}) does not handle the reload signal right now, so it was \
             not sent one; run Reload config in it, or the change applies when you restart \
             it.{then}"
        ),
        SignalOutcome::Failed { pid, reason } if problems_remain => format!(
            "The change is saved, but the running dux could not be told to reload ({reason}); \
             if it is a kind that refuses this file, it would keep its current settings anyway \
             until the problems above are fixed. {}",
            by_hand(pid)
        ),
        SignalOutcome::Failed { pid, reason } => format!(
            "The change is saved, but the running dux could not be told to reload ({reason}). {}",
            by_hand(pid)
        ),
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

/// The most read from a pipe: the largest password any setting allows, plus a
/// line break. More than that is refused rather than buffered.
const STDIN_LIMIT: u64 = dux_core::config_auth::MAX_PASSWORD_BYTES_LIMIT as u64 + 2;

/// Read a password from `input`: everything up to end of input, minus one
/// trailing `\n` or `\r\n`, in a buffer wiped on drop.
fn read_secret(input: impl Read) -> Result<Password> {
    let mut bytes = Zeroizing::new(Vec::new());
    input
        .take(STDIN_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow!("could not read the password from standard input: {e}"))?;
    if bytes.len() as u64 > STDIN_LIMIT {
        bail!("standard input held more than any password dux accepts; nothing was changed");
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
    fn stdin_beyond_any_allowed_password_is_refused() {
        let huge = "x".repeat(STDIN_LIMIT as usize + 10);
        assert!(read_secret(huge.as_bytes()).is_err());
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
        let sent = reload_sentence(&SignalOutcome::Sent { pid: 42 }, false);
        assert!(sent.contains("PID 42"), "{sent}");
        assert!(
            !sent.contains("live now"),
            "only the running dux knows: {sent}"
        );
        assert!(
            sent.contains("status line") && sent.contains("dux.log"),
            "{sent}"
        );
        assert!(reload_sentence(&SignalOutcome::NotRunning, false).contains("next time it starts"));
        let failed = reload_sentence(
            &SignalOutcome::Failed {
                pid: Some(7),
                reason: "permission denied".to_string(),
            },
            false,
        );
        assert!(failed.contains("kill -USR1 7"), "{failed}");
        // With problems left, neither "applied" sentence is said.
        for outcome in [
            SignalOutcome::Sent { pid: 42 },
            SignalOutcome::NotRunning,
            SignalOutcome::Failed {
                pid: Some(7),
                reason: "permission denied".to_string(),
            },
        ] {
            let said = reload_sentence(&outcome, true);
            assert!(!said.contains("next time it starts"), "{said}");
            assert!(
                said.contains("until the problems above are fixed"),
                "{said}"
            );
        }
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
    /// default (password-less) file and tell that dux to reload it.
    #[test]
    fn set_refuses_a_missing_config_while_a_dux_is_running() {
        let (_tmp, paths) = setup(None);
        let _running =
            dux_core::lockfile::SingleInstanceLock::acquire(&paths.lock_path).expect("lock");
        let err = set(&paths, &["server.port", "4000"], &mut no_secrets()).expect_err("refused");
        let text = err.to_string();
        assert!(text.contains("Recover config"), "{text}");
        assert!(!paths.config_path.exists(), "nothing was written");
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
        assert!(
            said.contains("A running terminal UI keeps the old password"),
            "{said}"
        );
        assert!(
            said.contains("A running dux server puts it in force"),
            "{said}"
        );
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

    /// A dux with no reload handler is never signalled, and is said to be an
    /// older version whose change applies when it restarts.
    #[test]
    fn an_older_dux_is_told_apart_and_never_promised_a_reload() {
        let said = reload_sentence(
            &SignalOutcome::NotSignalled {
                pid: 42,
                why: NoReloadHandler::OlderVersion,
            },
            false,
        );
        assert_eq!(
            said,
            "The running dux (PID 42) is an older version that cannot reload its config while \
             running; the change applies when you restart it."
        );
        let said = reload_sentence(
            &SignalOutcome::NotSignalled {
                pid: 42,
                why: NoReloadHandler::NotCaught,
            },
            true,
        );
        assert!(said.contains("not sent one"), "{said}");
        assert!(!said.contains("kill -USR1"), "{said}");
        // A holder that could not be identified is never offered `kill`.
        let said = reload_sentence(
            &SignalOutcome::Failed {
                pid: None,
                reason: "x".to_string(),
            },
            false,
        );
        assert!(!said.contains("kill -USR1"), "{said}");
    }
}
