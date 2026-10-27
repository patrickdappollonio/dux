//! The commands that run as a client of a dux: they reach it through
//! `dux_core::client`, print what it answers and exit with the client's codes.

use dux_core::client::config_resources::{self as resources, Source, Writer};
use dux_core::client::connect::{self, REMOTE_VARIABLE, Target};
use dux_core::client::output::{self, Shape};
use dux_core::client::remotes::{self, Remotes};
use dux_core::client::{CliError, Exit, server_inspect, sign_in, wait, workspace};
use dux_core::config::DuxPaths;
use dux_core::config_resources::StoppedDux;
use dux_tui::SecretInput;
use std::fmt::Write as _;
use std::io::Write;

use crate::commands::{
    AddAgentArgs, AgentsSub, ChangeFlags, ConnectionsSub, EnvSub, Format, GuardedChangeFlags,
    ListFlags, ListOnlySub, MacrosSub, NamedReadSub, OperationsSub, ProjectsSub, RemoteSource,
    RemoteSub, ServerSub, TabsSub, TerminalsSub, WorktreesSub,
};

/// What `--remote` and `--local` said, before the variable and the saved
/// default are consulted.
pub(crate) struct Selection {
    pub remote: Option<String>,
    pub local: bool,
}

impl Selection {
    /// What `--local` said, for the client.
    pub(crate) fn local_flag(&self) -> connect::LocalFlag {
        if self.local {
            connect::LocalFlag::Given
        } else {
            connect::LocalFlag::Absent
        }
    }

    fn variable() -> Option<String> {
        std::env::var(REMOTE_VARIABLE).ok()
    }

    /// [`crate::commands::check_config_target`] for `remote`, the name this
    /// selection resolved, with what selected it.
    pub(crate) fn check_config_target(&self, remote: Option<&str>) -> Result<(), String> {
        crate::commands::check_config_target(
            remote.map(|name| (name, self.remote_source())),
            self.local_flag(),
        )
    }

    /// What selects the remote when one is selected: the flag, else the variable,
    /// else the saved default, in the order [`Self::remote_name`] reads them.
    fn remote_source(&self) -> RemoteSource {
        if self.remote.is_some() {
            RemoteSource::Flag
        } else if Self::variable().is_some_and(|v| !v.is_empty()) {
            RemoteSource::Variable
        } else {
            RemoteSource::SavedDefault
        }
    }

    /// The remote this selection names, reading the saved default only when
    /// nothing else names one.
    pub(crate) fn remote_name(&self, paths: &DuxPaths) -> Result<Option<String>, CliError> {
        let variable = Self::variable();
        if self.local || self.remote.is_some() || variable.as_deref().is_some_and(|v| !v.is_empty())
        {
            return Ok(connect::selected_remote(
                self.remote.as_deref(),
                self.local_flag(),
                variable.as_deref(),
                None,
            ));
        }
        Ok(Remotes::load(&paths.root)?.default)
    }

    /// [`Self::remote_name`] for the `dux config` command `args`. A reset goes ahead
    /// past a remotes.toml it cannot read, so a broken one cannot stop the reset that
    /// removes it; a saved default it can read is a selected remote like any other.
    /// Any other command that cannot read it says so, with the ways on.
    pub(crate) fn config_remote_name(
        &self,
        paths: &DuxPaths,
        args: &[String],
    ) -> Result<Option<String>, CliError> {
        let selected = self.remote_name(paths);
        if args.first().is_some_and(|word| word == "reset") {
            // `remote_name` reads remotes.toml only when nothing else names a
            // remote, so a read it could not make selects none.
            return Ok(selected.unwrap_or(None));
        }
        selected.map_err(|error| {
            CliError::new(
                error.exit,
                format!(
                    "{}. dux config reads that file to learn whether a remote is selected: run \
                     \"dux --local config …\" to go ahead without it, or fix the file (\"dux \
                     config reset\" removes it, along with config.toml and the logs)",
                    error.message
                ),
            )
        })
    }

    fn target(&self, paths: &DuxPaths) -> Result<Target, CliError> {
        let remotes = Remotes::load(&paths.root)?;
        connect::choose_target(
            self.remote.as_deref(),
            self.local_flag(),
            Self::variable().as_deref(),
            &remotes,
        )
    }
}

/// Print a command's result, or its error, and end the process with its code.
pub(crate) fn finish(result: Result<String, CliError>) -> ! {
    finish_with_code(result.map(|text| (text, 0)))
}

/// [`finish`] for a command that writes to standard output and standard error as it runs: `run`
/// is handed both, locked, and they are let go before the result is printed.
pub(crate) fn finish_with(
    run: impl FnOnce(
        &mut std::io::StdoutLock<'static>,
        &mut std::io::StderrLock<'static>,
    ) -> Result<String, CliError>,
) -> ! {
    let result = run(&mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    finish(result)
}

/// [`finish`] for a result that carries its own exit code.
pub(crate) fn finish_with_code(result: Result<(String, i32), CliError>) -> ! {
    let code = write_outcome(
        result,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}

/// Write a command's result to `out`, or its error's sentence to `err`, and return the exit code
/// the command ends with. A reader that went away (`| head`) ends it quietly with its own code;
/// any other failure to write the result is said on `err` and ends it as failed.
fn write_outcome(
    result: Result<(String, i32), CliError>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    match result {
        Ok((text, code)) => match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
            Ok(()) => code,
            Err(error) => match dux_core::client::print_failure(&error) {
                None => code,
                Some(failed) => {
                    note(err, &failed.message);
                    failed.exit.code()
                }
            },
        },
        Err(error) => {
            note(err, &error.message);
            error.exit.code()
        }
    }
}

/// Write `text` as a line on `err`. A stderr that cannot take it has nowhere left to say so, and
/// a sentence that could not be shown is no reason to stop the command, so a failure is dropped.
pub(crate) fn note(err: &mut impl Write, text: &str) {
    let _ = writeln!(err, "{text}");
}

fn discover() -> Result<DuxPaths, CliError> {
    DuxPaths::discover().map_err(|error| CliError::new(Exit::Failed, format!("{error:#}")))
}

fn shape(list: &ListFlags) -> Shape {
    let format = if matches!(list.format, Format::Json) {
        Shape::Json
    } else {
        Shape::Table
    };
    let ids_only = if list.quiet {
        output::IdsOnly::Yes
    } else {
        output::IdsOnly::No
    };
    Shape::of(format, ids_only)
}

fn line(text: &str) -> String {
    format!("{text}\n")
}

/// `dux operations show`: the record on stdout, and the exit code its state
/// calls for (0 succeeded, 1 failed or partly done, 6 still running).
pub(crate) fn operations(
    command: OperationsSub,
    selection: &Selection,
) -> Result<(String, i32), CliError> {
    let paths = discover()?;
    let client = connect::connect(&selection.target(&paths)?, &paths.lock_path)?;
    match command {
        OperationsSub::Show { id } => {
            let record = client.operation(&id)?;
            let code = wait::exit_of(&record).map_or(0, Exit::code);
            Ok((line(&wait::describe(&record)), code))
        }
    }
}

/// `dux remote`: the saved remotes, and signing in to and out of one. Sentences that are not the
/// result (a plain HTTP warning, a sign-in that could not be ended) go to `err`.
pub(crate) fn remote(
    command: RemoteSub,
    selection: &Selection,
    err: &mut impl Write,
) -> Result<String, CliError> {
    let paths = discover()?;
    let root = &paths.root;
    match command {
        RemoteSub::Add {
            name,
            url,
            insecure,
        } => {
            let plain_http = if insecure {
                remotes::PlainHttp::Anywhere
            } else {
                remotes::PlainHttp::SafeNetworksOnly
            };
            remote_add(root, &name, &url, plain_http)
        }
        RemoteSub::Ls(list) => Ok(output::render(
            &Remotes::load(root)?.listing(),
            shape(&list),
        )),
        RemoteSub::Rm { name } => remote_rm(root, &name),
        RemoteSub::Default { name, unset: _ } => remote_default(root, name),
        RemoteSub::Login { name, stdin } => {
            let saved = Remotes::load(root)?;
            let name = named_or_selected(name, selection, &saved, "login")?;
            let remote = saved.get(&name)?.clone();
            let found = sign_in::look(&name, &remote)?;
            if !found.password_set {
                // Nothing to sign in to, but this is the dux the remote means
                // from now on.
                Remotes::update(root, |saved| {
                    if let Some(entry) = saved.remotes.get_mut(&name)
                        && entry.url == remote.url
                    {
                        entry.instance.clone_from(&found.instance);
                    }
                    Ok(())
                })?;
                return Ok(line(&format!(
                    "{name} has no password, so there is nothing to sign in to: commands reach it \
                     as they are."
                )));
            }
            if let Some(warning) = remotes::insecure_login_warning(&name, &remote) {
                note(err, &warning);
            }
            let input = if stdin {
                SecretInput::Stdin
            } else {
                SecretInput::Prompt
            };
            let password = dux_tui::read_sign_in_password(input, &format!("Password for {name}"))
                .map_err(|error| CliError::new(Exit::Failed, format!("{error:#}")))?;
            let signed_in = sign_in::login(&name, &remote, password.expose())?;
            // The dux that answered the sign-in is the one the remote means now.
            let instance = sign_in::pin_after_login(&name, &remote, &found)?;
            if let Some(kept) = &signed_in.old_sign_in_kept {
                note(err, kept);
            }
            // The prompt may have waited while another command changed the
            // remote; the token is kept only for the URL that issued it.
            Remotes::keep_token(
                root,
                &name,
                &remote.url,
                &signed_in.token,
                instance.as_deref(),
            )?;
            Ok(line(&format!("Signed in to {name}.")))
        }
        RemoteSub::Logout { name } => remote_logout(root, name, selection),
    }
}

/// `dux remote add`.
fn remote_add(
    root: &std::path::Path,
    name: &str,
    url: &str,
    plain_http: remotes::PlainHttp,
) -> Result<String, CliError> {
    let url = Remotes::update(root, |saved| {
        saved.add(name, url, plain_http)?;
        Ok(saved.get(name)?.url.clone())
    })?;
    Ok(line(&format!(
        "Saved remote {name} ({url}). Use it with --remote {name}, or make it the default \
         with \"dux remote default {name}\"."
    )))
}

/// `dux remote rm`.
fn remote_rm(root: &std::path::Path, name: &str) -> Result<String, CliError> {
    let (was_default, removed) = Remotes::update(root, |saved| {
        let was_default = saved.default.as_deref() == Some(name);
        Ok((was_default, saved.remove(name)?))
    })?;
    let mut text = format!("Forgot remote {name}.");
    if was_default {
        text.push_str(
            " It was the default, so commands now talk to this machine's dux unless \
             --remote or DUX_REMOTE names a remote.",
        );
    }
    if removed.token.is_some() {
        let _ = write!(
            text,
            " Its sign-in was discarded here without telling {name}, which ends it once \
             it goes unused for [server.auth] cli_token_idle_days."
        );
    }
    Ok(line(&text))
}

/// `dux remote default`: `name`, or none with `--unset`.
fn remote_default(root: &std::path::Path, name: Option<String>) -> Result<String, CliError> {
    let before = Remotes::update(root, |saved| {
        let before = saved.default.clone();
        saved.set_default(name.as_deref())?;
        Ok(before)
    })?;
    Ok(line(&match (name, before) {
        (Some(name), _) => format!(
            "{name} is now the default remote: commands talk to it unless --local, \
             --remote or DUX_REMOTE says otherwise."
        ),
        (None, None) => "No remote was the default, and none is now.".to_string(),
        (None, Some(old)) => format!(
            "{old} is no longer the default remote: commands talk to this machine's dux \
             unless --remote or DUX_REMOTE names a remote."
        ),
    }))
}

/// `dux remote logout`.
fn remote_logout(
    root: &std::path::Path,
    name: Option<String>,
    selection: &Selection,
) -> Result<String, CliError> {
    let saved = Remotes::load(root)?;
    let name = named_or_selected(name, selection, &saved, "logout")?;
    let remote = saved.get(&name)?.clone();
    let Some(token) = remote.token.clone() else {
        return Ok(line(&format!(
            "Not signed in to {name}, so there was nothing to end."
        )));
    };
    let told = sign_in::logout(&name, &remote, &token);
    Remotes::update(root, |saved| {
        if let Some(entry) = saved.remotes.get_mut(&name)
            && entry.token.as_deref() == Some(token.as_str())
        {
            entry.token = None;
        }
        Ok(())
    })?;
    match told {
        Ok(()) => Ok(line(&format!("Signed out of {name}."))),
        Err(error) => Err(CliError::new(
            error.exit,
            format!(
                "Forgot the sign-in to {name} here, but could not tell {name} to end it: \
                 {}. It ends on its own once it goes unused for [server.auth] \
                 cli_token_idle_days.",
                error.message
            ),
        )),
    }
}

/// The remote a login or logout is about: the one named, else the one
/// `--remote`, `DUX_REMOTE` or the default selects.
fn named_or_selected(
    name: Option<String>,
    selection: &Selection,
    saved: &Remotes,
    verb: &str,
) -> Result<String, CliError> {
    if let Some(name) = name {
        return Ok(name);
    }
    connect::selected_remote(
        selection.remote.as_deref(),
        selection.local_flag(),
        Selection::variable().as_deref(),
        saved.default.as_deref(),
    )
    .ok_or_else(|| {
        CliError::new(
            Exit::Usage,
            format!("name the remote: \"dux remote {verb} <name>\""),
        )
    })
}

// ---------------------------------------------------------------------------
// The resources kept in config.toml
// ---------------------------------------------------------------------------

/// Run a listing against this machine's config.toml, or a remote's API when
/// one is selected. The file is read whether or not dux is running.
fn read_with<T>(
    selection: &Selection,
    read: impl FnOnce(&Source<'_>) -> Result<T, CliError>,
) -> Result<T, CliError> {
    // `dux keys ls` reads the terminal UI's own bindings.
    dux_tui::install_canonical_renderer();
    let paths = discover()?;
    match selection.target(&paths)? {
        Target::Local => read(&Source::File(&paths)),
        target @ Target::Remote { .. } => {
            let client = connect::connect(&target, &paths.lock_path)?;
            read(&Source::Dux(&client))
        }
    }
}

/// Whether standard input is free to carry the answer to a confirmation.
#[derive(Clone, Copy)]
enum Stdin {
    /// Nothing else reads standard input.
    Free,
    /// Standard input carries a value, so it cannot also carry the answer.
    CarriesValue,
}

/// Run a change through the selected running dux, or on config.toml when this machine runs
/// none. `stdin` says whether standard input is free to carry the answer.
fn change_with(
    selection: &Selection,
    flags: &ChangeFlags,
    question: &str,
    stdin: Stdin,
    change: impl FnOnce(Writer<'_>) -> Result<String, CliError>,
) -> Result<String, CliError> {
    let paths = discover()?;
    let target = selection.target(&paths)?;
    let client = match client_if_running(&target, &paths)? {
        Some(client) => client,
        None => match StoppedDux::hold(&paths) {
            // Held across the question and the edit, so no dux starts in between.
            Ok(stopped) => {
                ask(
                    question,
                    &paths.config_path.display().to_string(),
                    flags.consent(),
                    stdin,
                )?;
                return change(Writer::File(&stopped));
            }
            Err(dux_core::lockfile::AcquireError::AlreadyRunning { .. }) => {
                client_if_running(&target, &paths)?.ok_or_else(|| {
                    CliError::new(
                        Exit::NotRunning,
                        "dux started while this command ran and does not answer yet; try again \
                         in a moment",
                    )
                })?
            }
            Err(error) => {
                return Err(CliError::new(
                    Exit::Failed,
                    format!(
                        "could not keep dux from starting while {} is edited, so nothing was \
                         changed: {error}",
                        paths.config_path.display()
                    ),
                ));
            }
        },
    };
    ask(question, client.target(), flags.consent(), stdin)?;
    let wait = if flags.no_wait {
        None
    } else {
        Some(wait::wait_timeout(flags.wait_timeout, &paths.config_path))
    };
    change(Writer::Dux {
        client: &client,
        wait,
    })
}

/// The selected dux, or `None` when it is this machine's and none is running.
fn client_if_running(
    target: &Target,
    paths: &DuxPaths,
) -> Result<Option<connect::Client>, CliError> {
    match target {
        Target::Local => connect::connect_local_if_running(&paths.lock_path),
        Target::Remote { .. } => connect::connect(target, &paths.lock_path).map(Some),
    }
}

/// Confirm a change on the terminal, or take `--yes` for it.
fn ask(
    question: &str,
    target: &str,
    consent: output::Consent,
    stdin: Stdin,
) -> Result<(), CliError> {
    use std::io::IsTerminal;
    let terminal = matches!(stdin, Stdin::Free)
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal();
    if !terminal {
        return output::confirm(question, target, consent, None);
    }
    let mut input = std::io::stdin().lock();
    let mut prompt = std::io::stderr();
    output::confirm(question, target, consent, Some((&mut input, &mut prompt)))
}

pub(crate) fn macros(command: MacrosSub, selection: &Selection) -> Result<String, CliError> {
    match command {
        MacrosSub::Ls(list) => read_with(selection, |source| {
            resources::macros_ls(source, shape(&list))
        }),
        MacrosSub::Show { name } => {
            read_with(selection, |source| resources::macros_show(source, &name))
        }
        MacrosSub::Add {
            name,
            text,
            surface,
            change,
        } => change_with(
            selection,
            &change,
            &resources::set_macro_question(&name),
            Stdin::Free,
            |writer| resources::set_macro(writer, &name, text, &surface),
        ),
        MacrosSub::Rm { name, change } => change_with(
            selection,
            &change,
            &resources::remove_macro_question(&name),
            Stdin::Free,
            |writer| resources::remove_macro(writer, &name),
        ),
    }
}

pub(crate) fn providers(command: NamedReadSub, selection: &Selection) -> Result<String, CliError> {
    match command {
        NamedReadSub::Ls(list) => read_with(selection, |source| {
            resources::providers_ls(source, shape(&list))
        }),
        NamedReadSub::Show { name } => {
            read_with(selection, |source| resources::providers_show(source, &name))
        }
    }
}

pub(crate) fn keys(command: ListOnlySub, selection: &Selection) -> Result<String, CliError> {
    let ListOnlySub::Ls(list) = command;
    read_with(selection, |source| resources::keys_ls(source, shape(&list)))
}

pub(crate) fn themes(command: ListOnlySub, selection: &Selection) -> Result<String, CliError> {
    let ListOnlySub::Ls(list) = command;
    read_with(selection, |source| {
        resources::themes_ls(source, shape(&list))
    })
}

pub(crate) fn env(command: EnvSub, selection: &Selection) -> Result<String, CliError> {
    match command {
        EnvSub::Ls { show, list } => read_with(selection, |source| {
            let values = if show {
                dux_core::config_keys::HeldBack::Shown
            } else {
                dux_core::config_keys::HeldBack::Masked
            };
            resources::env_ls(source, values, shape(&list))
        }),
        EnvSub::Set {
            name,
            stdin,
            change,
        } => {
            // A name that is no variable name is refused before anything is
            // asked or sent, so it never reaches a dux or its access log.
            resources::check_env_name(&name)?;
            let (input, answer_stdin) = if stdin {
                (SecretInput::Stdin, Stdin::CarriesValue)
            } else {
                (SecretInput::Prompt, Stdin::Free)
            };
            change_with(
                selection,
                &change,
                &resources::set_env_question(&name),
                answer_stdin,
                |writer| {
                    let value = dux_tui::read_env_value(input, &name)
                        .map_err(|error| CliError::new(Exit::Failed, format!("{error:#}")))?;
                    resources::set_env(writer, &name, value.expose())
                },
            )
        }
        EnvSub::Rm { name, change } => change_with(
            selection,
            &change,
            &resources::remove_env_question(&name),
            Stdin::Free,
            |writer| resources::remove_env(writer, &name),
        ),
    }
}

// ---------------------------------------------------------------------------
// Projects, agents, tabs, worktrees and terminals: always a running dux
// ---------------------------------------------------------------------------

/// The selected dux, which must be running: these resources live in its
/// database and engine, never in a file this command could read instead.
fn running(selection: &Selection) -> Result<(DuxPaths, connect::Client), CliError> {
    let paths = discover()?;
    let client = connect::connect(&selection.target(&paths)?, &paths.lock_path)?;
    Ok((paths, client))
}

fn read_on_dux(
    selection: &Selection,
    read: impl FnOnce(&connect::Client) -> Result<String, CliError>,
) -> Result<String, CliError> {
    let (_, client) = running(selection)?;
    read(&client)
}

/// Plan a change on the running dux, ask about it, send it and, unless told
/// not to, wait for how it ended.
fn change_on_dux(
    selection: &Selection,
    err: &mut impl Write,
    flags: &ChangeFlags,
    plan: impl FnOnce(&connect::Client, &std::path::Path) -> Result<workspace::Planned, CliError>,
) -> Result<String, CliError> {
    let (paths, client) = running(selection)?;
    let cwd = std::env::current_dir().map_err(|error| {
        CliError::new(
            Exit::Failed,
            format!("could not read the current folder: {error}"),
        )
    })?;
    let planned = plan(&client, &cwd)?;
    if let Some(notice) = &planned.notice {
        note(err, notice);
    }
    ask(
        &planned.question,
        client.target(),
        flags.consent(),
        Stdin::Free,
    )?;
    let wait = if flags.no_wait {
        None
    } else {
        Some(wait::wait_timeout(flags.wait_timeout, &paths.config_path))
    };
    workspace::perform(&client, &planned, wait)
}

/// A guarded change: the same, going ahead over whoever is attached only
/// when asked to in so many words.
fn guarded_on_dux(
    selection: &Selection,
    err: &mut impl Write,
    flags: &GuardedChangeFlags,
    plan: impl FnOnce(&connect::Client, workspace::IfAttached) -> Result<workspace::Planned, CliError>,
) -> Result<String, CliError> {
    let force = if flags.dangerously_ignore_connected {
        workspace::IfAttached::GoAhead
    } else {
        workspace::IfAttached::Refuse
    };
    change_on_dux(selection, err, &flags.change, |client, _| {
        plan(client, force)
    })
}

pub(crate) fn projects(
    command: ProjectsSub,
    selection: &Selection,
    err: &mut impl Write,
) -> Result<String, CliError> {
    match command {
        ProjectsSub::Ls(list) => read_on_dux(selection, |client| {
            workspace::projects_ls(client, shape(&list))
        }),
        ProjectsSub::Show { project } => read_on_dux(selection, |client| {
            workspace::projects_show(client, &project)
        }),
        ProjectsSub::Add {
            path,
            name,
            checkout_default,
            init,
            change,
        } => change_on_dux(selection, err, &change, |client, cwd| {
            let choices = workspace::ProjectAddChoices {
                checkout_default,
                init_repo: init,
            };
            workspace::projects_add(client, &path, name.as_deref(), choices, cwd)
        }),
        ProjectsSub::Rm {
            project,
            delete_worktrees,
            guarded,
        } => guarded_on_dux(selection, err, &guarded, |client, force| {
            let worktrees = if delete_worktrees {
                workspace::Worktrees::Delete
            } else {
                workspace::Worktrees::Keep
            };
            workspace::projects_rm(client, &project, worktrees, force)
        }),
        ProjectsSub::Worktrees {
            command: WorktreesSub::Ls { project, list },
        } => read_on_dux(selection, |client| {
            workspace::worktrees_ls(client, &project, shape(&list))
        }),
    }
}

pub(crate) fn agents(
    command: AgentsSub,
    selection: &Selection,
    err: &mut impl Write,
) -> Result<String, CliError> {
    match command {
        AgentsSub::Ls {
            project,
            worktrees,
            list,
        } => read_on_dux(selection, |client| {
            let columns = if worktrees {
                workspace::AgentColumns::Worktree
            } else {
                workspace::AgentColumns::All
            };
            workspace::agents_ls(client, project.as_deref(), columns, shape(&list))
        }),
        AgentsSub::Show { agent } => {
            read_on_dux(selection, |client| workspace::agents_show(client, &agent))
        }
        AgentsSub::Add(args) => {
            let how = new_agent(&args);
            change_on_dux(selection, err, &args.change, |client, cwd| {
                workspace::agents_add(client, how, args.name.as_deref(), cwd)
            })
            .map_err(workspace::existing_branch_refusal)
        }
        AgentsSub::Rm {
            agent,
            delete_worktree,
            delete_branch,
            keep_branch,
            guarded,
        } => {
            // Neither flag leaves it to how the branch came to be, as the
            // delete dialogs do.
            let branch = match (delete_branch, keep_branch) {
                (true, _) => Some(true),
                (_, true) => Some(false),
                _ => None,
            };
            let worktree = if delete_worktree {
                workspace::Worktrees::Delete
            } else {
                workspace::Worktrees::Keep
            };
            guarded_on_dux(selection, err, &guarded, |client, force| {
                workspace::agents_rm(client, &agent, worktree, branch, force)
            })
        }
        AgentsSub::Stop { agent, guarded } => {
            guarded_on_dux(selection, err, &guarded, |client, force| {
                workspace::agents_stop(client, &agent, force)
            })
        }
        AgentsSub::Start { agent, change } => {
            change_on_dux(selection, err, &change, |client, _| {
                workspace::agents_start(client, &agent)
            })
        }
        AgentsSub::Tabs { command } => tabs(command, selection, err),
    }
}

/// Which kind of create `dux agents add` was asked for. The command tree
/// guarantees exactly one source.
fn new_agent(args: &AddAgentArgs) -> workspace::NewAgent {
    let review_copy = if args.review_copy {
        workspace::ReviewCopy::Agreed
    } else {
        workspace::ReviewCopy::NotAgreed
    };
    if let Some(agent) = &args.fork {
        return workspace::NewAgent::Fork {
            agent: agent.clone(),
        };
    }
    if let Some(folder) = &args.standalone {
        return workspace::NewAgent::Standalone {
            folder: folder.clone(),
            provider: args.provider.clone(),
        };
    }
    let project = args.project.clone().unwrap_or_default();
    if let Some(reference) = &args.from_pr {
        if args.project.is_none() {
            return workspace::NewAgent::PullRequestAnywhere {
                reference: reference.clone(),
                path: args.path.clone(),
                review_copy,
            };
        }
        return workspace::NewAgent::PullRequest {
            project,
            reference: reference.clone(),
            review_copy,
        };
    }
    if let Some(path) = &args.from_worktree {
        return workspace::NewAgent::Worktree {
            project,
            path: path.clone(),
        };
    }
    workspace::NewAgent::Branch {
        project,
        existing_branch: args.existing_branch,
        copy_uncommitted: args.copy_uncommitted,
    }
}

fn tabs(command: TabsSub, selection: &Selection, err: &mut impl Write) -> Result<String, CliError> {
    match command {
        TabsSub::Ls { agent, list } => read_on_dux(selection, |client| {
            workspace::tabs_ls(client, &agent, shape(&list))
        }),
        TabsSub::Add {
            agent,
            provider,
            change,
        } => change_on_dux(selection, err, &change, |client, _| {
            workspace::tabs_add(client, &agent, provider.as_deref())
        }),
        TabsSub::Rm {
            agent,
            tab,
            guarded,
        } => guarded_on_dux(selection, err, &guarded, |client, force| {
            workspace::tabs_rm(client, &agent, &tab, force)
        }),
        TabsSub::Start { agent, tab, change } => {
            change_on_dux(selection, err, &change, |client, _| {
                workspace::tabs_start(client, &agent, &tab)
            })
        }
        TabsSub::Stop {
            agent,
            tab,
            guarded,
        } => guarded_on_dux(selection, err, &guarded, |client, force| {
            workspace::tabs_stop(client, &agent, &tab, force)
        }),
    }
}

pub(crate) fn terminals(
    command: TerminalsSub,
    selection: &Selection,
    err: &mut impl Write,
) -> Result<String, CliError> {
    match command {
        TerminalsSub::Ls(list) => read_on_dux(selection, |client| {
            workspace::terminals_ls(client, shape(&list))
        }),
        TerminalsSub::Add(args) => {
            let owner = match (&args.agent, &args.project) {
                (Some(agent), _) => workspace::TerminalFor::Agent(agent.clone()),
                (_, Some(project)) => workspace::TerminalFor::Project(project.clone()),
                (None, None) => workspace::TerminalFor::Standalone,
            };
            change_on_dux(selection, err, &args.change, |client, _| {
                workspace::terminals_add(client, owner)
            })
        }
        TerminalsSub::Rm { terminal, guarded } => {
            guarded_on_dux(selection, err, &guarded, |client, force| {
                workspace::terminals_rm(client, &terminal, force)
            })
        }
    }
}

// ---------------------------------------------------------------------------
// The server itself
// ---------------------------------------------------------------------------

/// `dux server connections ls`.
fn server_connections(command: ConnectionsSub, selection: &Selection) -> Result<String, CliError> {
    let ConnectionsSub::Ls(list) = command;
    let paths = discover()?;
    let client = connect::connect(&selection.target(&paths)?, &paths.lock_path)?;
    server_inspect::connections_ls(&client, shape(&list))
}

/// `dux server logs` and `dux server connections`. Log lines go to `out` as they are read, and
/// what is said about them to `err`.
pub(crate) fn server(
    command: ServerSub,
    selection: &Selection,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<String, CliError> {
    match command {
        ServerSub::Logs { follow, lines } => {
            let read = if follow {
                LogRead::Follow
            } else {
                LogRead::Tail
            };
            server_logs(read, lines, selection, out, err)
        }
        ServerSub::Connections { command } => server_connections(command, selection),
    }
}

/// How `dux server logs` reads the log.
#[derive(Clone, Copy)]
enum LogRead {
    /// Print the last lines and stop.
    Tail,
    /// Print the last lines, then every line after, until interrupted.
    Follow,
}

/// `dux server logs`: the last lines of the server log and, with `Follow`, every line after.
/// With no dux running on this machine, its log file is read as it stands.
fn server_logs(
    read: LogRead,
    lines: usize,
    selection: &Selection,
    out: &mut impl Write,
    err: &mut impl Write,
) -> Result<String, CliError> {
    let paths = discover()?;
    let mut print = |line: &str| print_line(out, line);
    let target = selection.target(&paths)?;
    // This machine's dux that writes no log leaves its file to be read; a
    // remote's file is not here, so its sentence is shown.
    let this_machine = matches!(target, Target::Local).then_some(&paths);
    match client_if_running(&target, &paths)? {
        Some(client) if matches!(read, LogRead::Follow) => {
            server_inspect::follow_or_file(&client, this_machine, lines, &mut print)?;
        }
        Some(client) => print_tail(
            server_inspect::tail_or_file(&client, this_machine, lines)?,
            &mut print,
            err,
        ),
        None => {
            let path = server_inspect::log_file(&paths)?;
            if !path.exists() {
                note(
                    err,
                    &format!(
                        "There is no server log at {} yet: dux has not served from this machine.",
                        path.display()
                    ),
                );
            }
            if matches!(read, LogRead::Follow) {
                server_inspect::file_follow(&path, lines, &mut print)?;
            } else {
                print_tail(server_inspect::file_tail(&path, lines)?, &mut print, err);
            }
        }
    }
    Ok(String::new())
}

/// Write one log line to `out` at once, so a follow shows it as it arrives. Any failure, a closed
/// pipe (`| head`) above all, ends the printing quietly.
fn print_line(out: &mut impl Write, line: &str) -> std::ops::ControlFlow<()> {
    match writeln!(out, "{line}").and_then(|()| out.flush()) {
        Ok(()) => std::ops::ControlFlow::Continue(()),
        Err(_) => std::ops::ControlFlow::Break(()),
    }
}

/// Print a log's last lines, then on `err` the sentence saying they were cut short, if any.
fn print_tail(
    (tail, cut_short): (Vec<String>, Option<String>),
    print: &mut dyn FnMut(&str) -> std::ops::ControlFlow<()>,
    err: &mut impl Write,
) {
    for line in tail {
        if print(&line).is_break() {
            break;
        }
    }
    if let Some(cut_short) = cut_short {
        note(err, &cut_short);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writer that refuses every write with `kind`.
    struct Refusing(std::io::ErrorKind);

    impl Write for Refusing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.0, "refused"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_result_goes_to_stdout_as_it_is_and_an_error_to_stderr_as_a_line() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = write_outcome(Ok(("one\ntwo".to_string(), 6)), &mut out, &mut err);
        assert_eq!(
            (code, out.as_slice(), err.as_slice()),
            (6, &b"one\ntwo"[..], &b""[..])
        );

        let (mut out, mut err) = (Vec::new(), Vec::new());
        let refused = CliError::new(Exit::NotRunning, "dux isn't running".to_string());
        let code = write_outcome(Err(refused), &mut out, &mut err);
        assert_eq!(
            (code, out.as_slice(), err.as_slice()),
            (4, &b""[..], &b"dux isn't running\n"[..])
        );
    }

    #[test]
    fn a_reader_that_went_away_ends_quietly_and_any_other_failure_is_said() {
        let mut err = Vec::new();
        let mut gone = Refusing(std::io::ErrorKind::BrokenPipe);
        assert_eq!(
            write_outcome(Ok(("x".to_string(), 0)), &mut gone, &mut err),
            0
        );
        assert_eq!(err, Vec::<u8>::new());

        let mut full = Refusing(std::io::ErrorKind::StorageFull);
        assert_eq!(
            write_outcome(Ok(("x".to_string(), 0)), &mut full, &mut err),
            1
        );
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "could not print the result: refused\n"
        );
    }

    #[test]
    fn a_tail_prints_its_lines_then_says_on_stderr_that_it_was_cut_short() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut print = |line: &str| print_line(&mut out, line);
        print_tail(
            (
                vec!["one".to_string(), "two".to_string()],
                Some("Only the last 2 lines.".to_string()),
            ),
            &mut print,
            &mut err,
        );
        assert_eq!(out, b"one\ntwo\n");
        assert_eq!(err, b"Only the last 2 lines.\n");
    }

    #[test]
    fn a_tail_stops_at_the_first_line_its_reader_cannot_take() {
        let mut gone = Refusing(std::io::ErrorKind::BrokenPipe);
        let mut lines = 0;
        let mut print = |line: &str| {
            lines += 1;
            print_line(&mut gone, line)
        };
        let mut err = Vec::new();
        print_tail(
            (vec!["one".to_string(), "two".to_string()], None),
            &mut print,
            &mut err,
        );
        assert_eq!(lines, 1);
        assert_eq!(err, Vec::<u8>::new());
    }
}
