//! Asking the running dux on this machine to reload its config, and hearing
//! how that went.
//!
//! `dux config set` writes `config.toml` itself and then asks the dux that
//! holds `dux.lock` to read it again, over the control socket. The answer is
//! the reload's own operation record, so what the command says afterwards is
//! what the running dux really did, not that a request was sent.

use std::path::Path;
use std::time::Duration;

use super::connect::{Target, connect};
use super::transport::Method;
use super::wait::RecordState;
use super::{CliError, Exit};
use crate::reload_signal::{LockHolder, lock_holder};

/// How asking the local dux to reload ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReloadAnswer {
    /// No dux holds the lock: the change applies the next time one starts.
    NotRunning,
    /// The new settings are in force; the sentence is the running dux's own,
    /// and includes anything only a restart applies.
    Applied(String),
    /// The new config is in force, but one step of applying it failed.
    PartlyApplied(String),
    /// The running dux could not take the file on, so its settings are
    /// unchanged.
    Refused(String),
    /// A dux is running but could not be asked, or did not answer the request.
    NotReached(String),
    /// The reload was still running when the wait ended.
    Unknown(String),
}

/// Ask the dux holding `lock_path` to reload `config.toml` and wait up to
/// `wait` for how it went.
pub fn ask_to_reload(lock_path: &Path, wait: Duration) -> ReloadAnswer {
    let client = match connect(&Target::Local, lock_path) {
        Ok(client) => client,
        Err(error) if error.exit == Exit::NotRunning => {
            return match lock_holder(lock_path) {
                LockHolder::Free => ReloadAnswer::NotRunning,
                _ => ReloadAnswer::NotReached(error.message),
            };
        }
        Err(error) => return ReloadAnswer::NotReached(error.message),
    };
    let ended = client
        .change(Method::Post, "/api/v1/config/reload", None)
        .and_then(|record| client.wait(record, wait));
    match ended {
        Ok(record) => {
            let message = record.message;
            match record.state {
                RecordState::Succeeded => ReloadAnswer::Applied(message),
                RecordState::Partial => ReloadAnswer::PartlyApplied(message),
                RecordState::Failed => ReloadAnswer::Refused(message),
                RecordState::Running | RecordState::Unknown => ReloadAnswer::Unknown(message),
            }
        }
        Err(CliError {
            exit: Exit::Unknown,
            message,
        }) => ReloadAnswer::Unknown(message),
        Err(error) => ReloadAnswer::NotReached(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;

    fn record(state: &str, message: &str) -> String {
        format!(
            r#"{{"id":"op-9","kind":"config.reload","state":"{state}","message":"{message}","created":[],"removed":[],"parts":[]}}"#
        )
    }

    /// A stand-in dux holding the lock and answering on its socket: the
    /// reload request gets a running record, and reading it gets `outcome`.
    fn running_dux(dir: &Path, outcome: String) -> (FakeDux, crate::lockfile::SingleInstanceLock) {
        let socket = dir.join("dux.sock");
        let fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            path if path.starts_with("/api/v1/operations/op-9") => Reply::json(200, &outcome),
            "/api/v1/config/reload?operation=1" => Reply::json(202, &record("running", "")),
            _ => Reply::json(404, "{}"),
        });
        let lock_path = dir.join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        (fake, lock)
    }

    #[test]
    fn the_running_duxs_own_outcome_is_what_comes_back() {
        let cases = [
            (
                record("succeeded", "Configuration reloaded."),
                ReloadAnswer::Applied("Configuration reloaded.".to_string()),
            ),
            (
                record("partial", "One step failed."),
                ReloadAnswer::PartlyApplied("One step failed.".to_string()),
            ),
            (
                record("failed", "Config reload failed: bad toml"),
                ReloadAnswer::Refused("Config reload failed: bad toml".to_string()),
            ),
        ];
        for (outcome, expected) in cases {
            let dir = private_dir();
            let (fake, _lock) = running_dux(dir.path(), outcome);
            let answer = ask_to_reload(&dir.path().join("dux.lock"), Duration::from_secs(10));
            assert_eq!(answer, expected);
            assert!(
                fake.seen()
                    .iter()
                    .any(|s| s.method == "POST" && s.path == "/api/v1/config/reload?operation=1"),
                "{:?}",
                fake.seen()
            );
        }
    }

    #[test]
    fn a_reload_still_running_when_the_wait_ends_is_unknown() {
        let dir = private_dir();
        let (_fake, _lock) = running_dux(dir.path(), record("running", ""));
        let answer = ask_to_reload(&dir.path().join("dux.lock"), Duration::from_secs(1));
        let ReloadAnswer::Unknown(message) = answer else {
            panic!("expected an unknown outcome, got {answer:?}");
        };
        assert!(message.contains("op-9"), "{message}");
    }

    #[test]
    fn no_dux_running_is_said_apart_from_one_that_cannot_be_asked() {
        let dir = private_dir();
        let lock_path = dir.path().join("dux.lock");
        assert_eq!(
            ask_to_reload(&lock_path, Duration::from_secs(1)),
            ReloadAnswer::NotRunning
        );

        let _lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        let ReloadAnswer::NotReached(message) = ask_to_reload(&lock_path, Duration::from_secs(1))
        else {
            panic!("a held lock with no socket cannot be asked");
        };
        assert!(
            message.contains("is running but does not answer on its control socket"),
            "{message}"
        );
    }
}
