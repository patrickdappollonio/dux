//! The command line's client: how `dux <resource> …` reaches a running dux,
//! prints what it answers and says how a command ended.
//!
//! The client never opens `sessions.sqlite3`. Everything the database holds is
//! read and changed through a running dux, over a [`transport::Transport`]: the
//! control socket of the dux on this machine, or the web listeners of a saved
//! remote. A target that does not answer stops the command; nothing falls back
//! to another one.

pub mod connect;
pub mod output;
pub mod remotes;
pub mod sign_in;
pub mod transport;
pub mod wait;

#[cfg(test)]
mod test_server;

/// How a command ended, as its process exit code. Scripts read these, so a
/// number never changes meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The change failed, or what was asked for does not exist.
    Failed,
    /// The command line itself is wrong.
    Usage,
    /// Refused: someone is attached, another change is in the way, or a
    /// confirmation was declined or could not be asked.
    Refused,
    /// No dux is running, or it does not answer.
    NotRunning,
    /// The remote asks for its password and this client has no sign-in.
    PasswordNeeded,
    /// The change was sent but its outcome is not known yet.
    Unknown,
}

impl Exit {
    pub fn code(self) -> i32 {
        match self {
            Exit::Failed => 1,
            Exit::Usage => 2,
            Exit::Refused => 3,
            Exit::NotRunning => 4,
            Exit::PasswordNeeded => 5,
            Exit::Unknown => 6,
        }
    }
}

/// A command that did not succeed: the sentence for stderr and the exit code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError {
    pub exit: Exit,
    pub message: String,
}

impl CliError {
    pub fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// The version of the API this client speaks, compared with what
/// `GET /api/v1/build` reports.
pub const API_VERSION: u64 = 1;
