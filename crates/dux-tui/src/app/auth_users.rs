//! Web UI login-user management from the trusted TUI side.
//!
//! Two palette commands live here: `server-add-user` (a two-step username →
//! masked-password prompt) and `server-remove-user` (a picker over configured
//! usernames). Both persist `[auth] users` through the canonical config writer
//! so the section round-trips, and a running web server picks the change up on
//! its next config reload.
//!
//! User management is deliberately TUI/config only — never the web (the Locked
//! design's scope cut: a remote web session minting login users would be a
//! privilege-escalation surface).
//!
//! Hashing is bcrypt at the default cost (~250ms), which would freeze the UI if
//! run inline, so the add path hashes AND writes config on a background thread
//! and reports completion via [`WorkerEvent::AuthUsersPersisted`] (per the
//! workers tenet).

use super::*;

use crate::keybindings::RuntimeBindings;
use dux_core::auth;
use dux_core::engine::InFlightKey;

/// Status error shown when an add/remove is opened while another persist is
/// still in flight (the bcrypt hash + config write run off-thread).
const AUTH_USERS_IN_FLIGHT_MESSAGE: &str =
    "A login-user change is already in progress — wait for it to finish.";

impl App {
    /// True while a login-user add/remove is persisting. Opening either prompt
    /// is refused in this window so two operations can't start from the same
    /// stale `[auth] users` snapshot and silently drop one writer's change.
    fn auth_users_persist_in_flight(&self) -> bool {
        self.engine.is_in_flight(&InFlightKey::AuthUsers)
    }

    /// Open step 1 of `server-add-user`: prompt for the username.
    pub(crate) fn open_server_add_user(&mut self) {
        if self.auth_users_persist_in_flight() {
            self.set_error(AUTH_USERS_IN_FLIGHT_MESSAGE);
            return;
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ServerAddUserName {
            input: TextInput::new().with_placeholder("username"),
        };
        self.set_info(
            "Add or update a web UI login user. Type a username, then press Enter to set a password.",
        );
    }

    /// Advance from the username step to the masked password step, validating
    /// the username first. Existing usernames are accepted — re-adding one
    /// changes that user's password (last-wins semantics from slice A1).
    pub(crate) fn confirm_server_add_user_name(&mut self) {
        let username = match &self.prompt {
            PromptState::ServerAddUserName { input } => input.text.trim().to_string(),
            _ => return,
        };
        if let Err(message) = auth::validate_username(&username) {
            self.set_error(message);
            return;
        }
        let is_update = auth::parse_users(&self.engine.config.auth.users)
            .iter()
            .any(|user| user.username == username);
        let note = if is_update {
            format!("Set a new password for existing user \"{username}\".")
        } else {
            format!("Set a password for new user \"{username}\".")
        };
        self.prompt = PromptState::ServerAddUserPassword {
            username,
            input: TextInput::new().masked().with_placeholder("password"),
            is_update,
        };
        self.set_info(note);
    }

    /// Finish `server-add-user`: validate the password length, then hash and
    /// persist on a background thread. Rejects (does not truncate) passwords
    /// longer than bcrypt's 72-byte limit so the user is never misled into
    /// thinking the dropped suffix protects the account (slice A1 obligation).
    pub(crate) fn confirm_server_add_user_password(&mut self) {
        let (username, password, is_update) = match &self.prompt {
            PromptState::ServerAddUserPassword {
                username,
                input,
                is_update,
            } => (username.clone(), input.text.clone(), *is_update),
            _ => return,
        };
        if password.is_empty() {
            self.set_error("Password cannot be empty.");
            return;
        }
        if !auth::password_within_bcrypt_limit(&password) {
            self.set_error(format!(
                "Password is too long ({} bytes). bcrypt only hashes the first {} bytes, so a longer password is no stronger — choose one of at most {} bytes.",
                password.len(),
                auth::MAX_BCRYPT_PASSWORD_BYTES,
                auth::MAX_BCRYPT_PASSWORD_BYTES,
            ));
            return;
        }

        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        self.set_busy(format!("Hashing the password for \"{username}\"\u{2026}"));
        self.spawn_auth_user_add(username, password, is_update);
    }

    /// Open `server-remove-user`: a picker over the configured usernames. Each
    /// distinct username appears once even if duplicate entries exist (removal
    /// deletes them all).
    pub(crate) fn open_server_remove_user(&mut self) {
        if self.auth_users_persist_in_flight() {
            self.set_error(AUTH_USERS_IN_FLIGHT_MESSAGE);
            return;
        }
        let usernames = distinct_usernames(&self.engine.config.auth.users);
        if usernames.is_empty() {
            self.set_error(
                "No web UI login users are configured. Use server-add-user to add one first.",
            );
            return;
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ServerRemoveUser {
            usernames,
            selected: 0,
        };
        self.set_info("Select a web UI login user to remove, then press Enter.");
    }

    /// Remove the highlighted user (every `[auth]` entry for that username) and
    /// persist on a background thread.
    pub(crate) fn confirm_server_remove_user(&mut self) {
        let username = match &self.prompt {
            PromptState::ServerRemoveUser {
                usernames,
                selected,
            } => match usernames.get(*selected) {
                Some(username) => username.clone(),
                None => return,
            },
            _ => return,
        };
        let remaining: Vec<String> = self
            .engine
            .config
            .auth
            .users
            .iter()
            .filter(|entry| {
                auth::parse_users(std::slice::from_ref(*entry))
                    .first()
                    .map(|user| user.username != username)
                    // Keep malformed entries (they aren't this user) untouched.
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        // Removing the last valid user turns the login gate OFF (auth_enabled
        // counts only valid entries). Warn loudly: on a non-loopback bind a
        // running server stays bound and becomes open once it reloads config.
        let no_users_left = auth::parse_users(&remaining).is_empty();
        let (message, warn) = remove_user_status(&username, no_users_left);
        self.set_busy(format!("Removing web UI login user \"{username}\"\u{2026}"));
        self.spawn_auth_users_persist(remaining, message, warn);
    }

    /// Append `username:hash` to the configured users (last-wins replaces an
    /// existing password) and persist. Hashing happens on the worker thread.
    fn spawn_auth_user_add(&mut self, username: String, password: String, is_update: bool) {
        // Set the single-flight guard at the spawn point; the engine's
        // AuthUsersPersisted arm clears it on completion (success and failure).
        self.engine.mark_in_flight(InFlightKey::AuthUsers);
        let mut users = self.engine.config.auth.users.clone();
        let config = self.engine.config.clone();
        let config_path = self.engine.paths.config_path.clone();
        let tx = self.engine.worker_tx.clone();
        std::thread::spawn(move || {
            let hash = match auth::hash_password(&password) {
                Ok(hash) => hash,
                Err(err) => {
                    let _ = tx.send(WorkerEvent::AuthUsersPersisted {
                        users,
                        message: String::new(),
                        warn: false,
                        result: Err(format!("could not hash the password: {err:#}")),
                    });
                    return;
                }
            };
            users.push(format!("{username}:{hash}"));
            let verb = if is_update { "updated" } else { "added" };
            let message = format!(
                "Web UI login user \"{username}\" {verb}. A running web server picks this up after reload-config; otherwise it applies on the next server start."
            );
            // An add never empties the list, so it is always an info-tone result.
            persist_auth_users(users, config, config_path, message, false, tx);
        });
    }

    /// Persist an already-computed users list (no hashing needed) on a worker
    /// thread. Used by removal.
    fn spawn_auth_users_persist(&mut self, users: Vec<String>, message: String, warn: bool) {
        // Set the single-flight guard at the spawn point; the engine's
        // AuthUsersPersisted arm clears it on completion (success and failure).
        self.engine.mark_in_flight(InFlightKey::AuthUsers);
        let config = self.engine.config.clone();
        let config_path = self.engine.paths.config_path.clone();
        let tx = self.engine.worker_tx.clone();
        std::thread::spawn(move || {
            persist_auth_users(users, config, config_path, message, warn, tx);
        });
    }
}

/// Write the updated `users` list to `config_path` via the canonical config
/// writer (which owns the `[auth]` section) and report the outcome. Runs on a
/// worker thread.
fn persist_auth_users(
    users: Vec<String>,
    mut config: Config,
    config_path: std::path::PathBuf,
    message: String,
    warn: bool,
    tx: std::sync::mpsc::Sender<WorkerEvent>,
) {
    config.auth.users = users.clone();
    let bindings = RuntimeBindings::from_keys_config(&config.keys);
    let result = crate::config::save_config(&config_path, &config, &bindings)
        .map_err(|err| format!("{err:#}"));
    let _ = tx.send(WorkerEvent::AuthUsersPersisted {
        users,
        message,
        warn,
        result,
    });
}

/// Build the status line (and warn tone) shown after removing a login user.
///
/// `no_users_left` is whether the removal emptied the valid-user set (the last
/// user). When true the message must reflect the S2 refuse-the-downgrade rule:
/// the TUI only edits config, so it cannot know the live server's bind — it
/// spells out BOTH outcomes. A loopback server turns the gate off on
/// reload-config; a non-loopback server REFUSES the downgrade and keeps the
/// previous users until it is restarted. The non-empty case is the ordinary
/// per-user revocation message (info tone).
fn remove_user_status(username: &str, no_users_left: bool) -> (String, bool) {
    if no_users_left {
        (
            format!(
                "Removed web UI login user \"{username}\". No users remain — authentication is now DISABLED in config. A running LOOPBACK web server turns the gate off on reload-config; a NON-LOOPBACK server REFUSES the downgrade and keeps the previous users until it is restarted."
            ),
            true,
        )
    } else {
        (
            format!(
                "Removed web UI login user \"{username}\". A running web server revokes that user's sessions after reload-config; otherwise it applies on the next server start."
            ),
            false,
        )
    }
}

/// The distinct usernames present in `users`, in first-seen order, skipping
/// malformed entries. Duplicates collapse to a single picker row even though
/// removal deletes every matching entry.
pub(crate) fn distinct_usernames(users: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for user in auth::parse_users(users) {
        if seen.insert(user.username.to_string()) {
            out.push(user.username.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{distinct_usernames, remove_user_status};

    #[test]
    fn remove_user_status_last_user_explains_loopback_vs_non_loopback() {
        let (message, warn) = remove_user_status("alice", true);
        assert!(warn, "removing the last user is a warn-tone event");
        assert!(message.contains("alice"));
        assert!(
            message.contains("DISABLED"),
            "must say auth becomes disabled in config: {message}"
        );
        // The S2 refuse-the-downgrade rule: both bind outcomes must be spelled
        // out (the TUI cannot know the live server's bind).
        assert!(
            message.to_lowercase().contains("loopback"),
            "must mention the loopback case: {message}"
        );
        assert!(
            message.contains("REFUSES the downgrade"),
            "must say a non-loopback server refuses the downgrade: {message}"
        );
        assert!(
            message.contains("restarted"),
            "must say a non-loopback server keeps users until restarted: {message}"
        );
        // The stale wording must be gone.
        assert!(
            !message.contains("A running web server applies this after reload-config"),
            "the old unconditional wording must be replaced: {message}"
        );
    }

    #[test]
    fn remove_user_status_non_last_user_is_info_tone_revocation() {
        let (message, warn) = remove_user_status("bob", false);
        assert!(!warn, "removing a non-last user is an info-tone event");
        assert!(message.contains("bob"));
        assert!(
            message.contains("revokes that user's sessions after reload-config"),
            "must describe per-user revocation: {message}"
        );
    }

    #[test]
    fn distinct_usernames_dedupes_in_first_seen_order() {
        let users = vec![
            "alice:$2b$12$aaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "bob:$2b$12$bbbbbbbbbbbbbbbbbbbbbb".to_string(),
            // duplicate alice (a password change via append) collapses to one row
            "alice:$2b$12$cccccccccccccccccccccc".to_string(),
        ];
        assert_eq!(distinct_usernames(&users), vec!["alice", "bob"]);
    }

    #[test]
    fn distinct_usernames_skips_malformed() {
        let users = vec![
            "no-colon".to_string(),
            ":emptyname".to_string(),
            "carol:$2b$12$dddddddddddddddddddddd".to_string(),
        ];
        assert_eq!(distinct_usernames(&users), vec!["carol"]);
    }

    #[test]
    fn distinct_usernames_empty() {
        assert!(distinct_usernames(&[]).is_empty());
    }
}
