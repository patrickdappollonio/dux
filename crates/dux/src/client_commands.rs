//! The commands that run as a client of a dux: they reach it through
//! `dux_core::client`, print what it answers and exit with the client's codes.

use dux_core::client::connect::{self, REMOTE_VARIABLE, Target};
use dux_core::client::output::{self, Shape};
use dux_core::client::remotes::{self, Remotes};
use dux_core::client::{CliError, Exit, sign_in, wait};
use dux_core::config::DuxPaths;

use crate::commands::{Format, ListFlags, OperationsSub, RemoteSub};

/// What `--remote` and `--local` said, before the variable and the saved
/// default are consulted.
pub struct Selection {
    pub remote: Option<String>,
    pub local: bool,
}

impl Selection {
    fn variable() -> Option<String> {
        std::env::var(REMOTE_VARIABLE).ok()
    }

    /// The remote this selection names, reading the saved default only when
    /// nothing else names one.
    pub fn remote_name(&self, paths: &DuxPaths) -> Result<Option<String>, CliError> {
        let variable = Self::variable();
        if self.local || self.remote.is_some() || variable.as_deref().is_some_and(|v| !v.is_empty())
        {
            return Ok(connect::selected_remote(
                self.remote.as_deref(),
                self.local,
                variable.as_deref(),
                None,
            ));
        }
        Ok(Remotes::load(&paths.root)?.default)
    }

    fn target(&self, paths: &DuxPaths) -> Result<Target, CliError> {
        let remotes = Remotes::load(&paths.root)?;
        connect::choose_target(
            self.remote.as_deref(),
            self.local,
            Self::variable().as_deref(),
            &remotes,
        )
    }
}

/// Print a command's result, or its error, and end the process with its code.
pub fn finish(result: Result<String, CliError>) -> ! {
    finish_with_code(result.map(|text| (text, 0)))
}

/// [`finish`] for a result that carries its own exit code.
pub fn finish_with_code(result: Result<(String, i32), CliError>) -> ! {
    match result {
        Ok((text, code)) => {
            print!("{text}");
            std::process::exit(code);
        }
        Err(error) => {
            eprintln!("{}", error.message);
            std::process::exit(error.exit.code());
        }
    }
}

fn discover() -> Result<DuxPaths, CliError> {
    DuxPaths::discover().map_err(|error| CliError::new(Exit::Failed, format!("{error:#}")))
}

fn shape(list: &ListFlags) -> Shape {
    Shape::of(matches!(list.format, Format::Json), list.quiet)
}

fn line(text: String) -> String {
    format!("{text}\n")
}

/// `dux operations show`: the record on stdout, and the exit code its state
/// calls for (0 succeeded, 1 failed or partly done, 6 still running).
pub fn operations(
    command: OperationsSub,
    selection: &Selection,
) -> Result<(String, i32), CliError> {
    let paths = discover()?;
    let client = connect::connect(&selection.target(&paths)?, &paths.lock_path)?;
    match command {
        OperationsSub::Show { id } => {
            let record = client.operation(&id)?;
            let code = wait::exit_of(&record).map_or(0, Exit::code);
            Ok((line(wait::describe(&record)), code))
        }
    }
}

pub fn remote(command: RemoteSub, selection: &Selection) -> Result<String, CliError> {
    let paths = discover()?;
    let root = &paths.root;
    match command {
        RemoteSub::Add {
            name,
            url,
            insecure,
        } => {
            let url = Remotes::update(root, |saved| {
                saved.add(&name, &url, insecure)?;
                Ok(saved.get(&name)?.url.clone())
            })?;
            Ok(line(format!(
                "Saved remote {name} ({url}). Use it with --remote {name}, or make it the default \
                 with \"dux remote default {name}\"."
            )))
        }
        RemoteSub::Ls(list) => Ok(output::render(
            &Remotes::load(root)?.listing(),
            shape(&list),
        )),
        RemoteSub::Rm { name } => {
            let (was_default, removed) = Remotes::update(root, |saved| {
                let was_default = saved.default.as_deref() == Some(name.as_str());
                Ok((was_default, saved.remove(&name)?))
            })?;
            let mut text = format!("Forgot remote {name}.");
            if was_default {
                text.push_str(
                    " It was the default, so commands now talk to this machine's dux unless \
                     --remote or DUX_REMOTE names a remote.",
                );
            }
            if removed.token.is_some() {
                text.push_str(&format!(
                    " Its sign-in was discarded here without telling {name}, which ends it once \
                     it goes unused for [server.auth] cli_token_idle_days."
                ));
            }
            Ok(line(text))
        }
        RemoteSub::Default { name, unset: _ } => {
            let before = Remotes::update(root, |saved| {
                let before = saved.default.clone();
                saved.set_default(name.as_deref())?;
                Ok(before)
            })?;
            Ok(line(match (name, before) {
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
        RemoteSub::Login { name, stdin } => {
            let saved = Remotes::load(root)?;
            let name = named_or_selected(name, selection, &saved, "login")?;
            let remote = saved.get(&name)?.clone();
            if !sign_in::password_set(&name, &remote)? {
                return Ok(line(format!(
                    "{name} has no password, so there is nothing to sign in to: commands reach it \
                     as they are."
                )));
            }
            if let Some(warning) = remotes::insecure_login_warning(&name, &remote) {
                eprintln!("{warning}");
            }
            let password = dux_tui::read_sign_in_password(stdin, &format!("Password for {name}"))
                .map_err(|error| CliError::new(Exit::Failed, format!("{error:#}")))?;
            let token = sign_in::login(&name, &remote, password.expose())?;
            // The prompt may have waited while another command changed the
            // remote; the token is kept only for the URL that issued it.
            Remotes::keep_token(root, &name, &remote.url, &token)?;
            Ok(line(format!("Signed in to {name}.")))
        }
        RemoteSub::Logout { name } => {
            let saved = Remotes::load(root)?;
            let name = named_or_selected(name, selection, &saved, "logout")?;
            let remote = saved.get(&name)?.clone();
            let Some(token) = remote.token.clone() else {
                return Ok(line(format!(
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
                Ok(()) => Ok(line(format!("Signed out of {name}."))),
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
        selection.local,
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
