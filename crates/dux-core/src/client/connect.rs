//! Choosing the dux a command talks to, and reaching it.
//!
//! The target is `--local`, else `--remote <name>`, else `DUX_REMOTE`, else
//! the saved default, else the dux on this machine. Whatever is chosen is the
//! only one tried: a remote that does not answer is reported, never swapped
//! for the local dux, and the other way round.
//!
//! The dux on this machine is found through `dux.lock`: the socket its holder
//! names there is tried first, and only when nothing answers on it is the
//! lock itself examined, to say why.

use std::path::Path;

use serde::de::DeserializeOwned;

use super::remotes::{Remote, Remotes};
use super::transport::{
    HttpTransport, Request, Response, Transport, TransportError, UnixTransport,
};
use super::{API_VERSION, CliError, Exit};
use crate::lockfile::LockFileContents;
use crate::reload_signal::LockHolder;

/// The variable that names a remote for every command, unless `--local` or
/// `--remote` says otherwise.
pub const REMOTE_VARIABLE: &str = "DUX_REMOTE";

/// The dux a command talks to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Local,
    Remote { name: String, remote: Remote },
}

/// The name of the remote selected by `flag`, `env` (the value of
/// [`REMOTE_VARIABLE`]) or the saved default, in that order; `None` with
/// `--local` or when nothing names one. An empty variable names nothing.
pub fn selected_remote(
    flag: Option<&str>,
    local: bool,
    env: Option<&str>,
    default: Option<&str>,
) -> Option<String> {
    if local {
        return None;
    }
    flag.or(env.filter(|name| !name.is_empty()))
        .or(default)
        .map(str::to_string)
}

/// The target a command talks to. A remote name nothing is saved under
/// stops the command.
pub fn choose_target(
    flag: Option<&str>,
    local: bool,
    env: Option<&str>,
    remotes: &Remotes,
) -> Result<Target, CliError> {
    match selected_remote(flag, local, env, remotes.default.as_deref()) {
        None => Ok(Target::Local),
        Some(name) => {
            let remote = remotes.get(&name)?.clone();
            Ok(Target::Remote { name, remote })
        }
    }
}

/// A dux that answered, ready for requests.
pub struct Client {
    transport: Box<dyn Transport>,
    token: Option<String>,
    target: String,
    remote: Option<String>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

/// Reach `target` and check that it speaks this client's API. `lock_path`
/// is this machine's `dux.lock`, read only for the local target.
pub fn connect(target: &Target, lock_path: &Path) -> Result<Client, CliError> {
    let client = match target {
        Target::Local => local_client(find_local(lock_path)?),
        Target::Remote { name, remote } => {
            let client = Client {
                transport: Box::new(HttpTransport::new(&remote.url, remote.insecure)),
                token: remote.token.clone(),
                target: format!("{name} ({})", remote.url),
                remote: Some(name.clone()),
            };
            client.check_sign_in()?;
            client
        }
    };
    client.check_api()?;
    Ok(client)
}

/// The dux on this machine when one is running, ready for requests; `None`
/// when none is (the lock is free), for a command that then works on the
/// config file itself. A dux that holds the lock but does not answer is
/// reported, never worked around.
pub fn connect_local_if_running(lock_path: &Path) -> Result<Option<Client>, CliError> {
    let Some(transport) = find_running_local(lock_path)? else {
        return Ok(None);
    };
    let client = local_client(transport);
    client.check_api()?;
    Ok(Some(client))
}

/// A client of the dux on this machine, which needs no sign-in.
fn local_client(transport: UnixTransport) -> Client {
    Client {
        transport: Box::new(transport),
        token: None,
        target: "this machine's dux".to_string(),
        remote: None,
    }
}

/// The control socket of the dux on this machine, or why there is none to
/// talk to.
fn find_local(lock_path: &Path) -> Result<UnixTransport, CliError> {
    find_running_local(lock_path)?.ok_or_else(|| {
        CliError::new(
            Exit::NotRunning,
            "dux isn't running; start it with \"dux\" or \"dux server\"",
        )
    })
}

/// [`find_local`], with `None` for a free lock.
fn find_running_local(lock_path: &Path) -> Result<Option<UnixTransport>, CliError> {
    let read = || {
        std::fs::read_to_string(lock_path)
            .map(|text| LockFileContents::parse(&text))
            .unwrap_or_default()
    };
    if let Some(socket) = read().control_socket {
        let transport = UnixTransport::new(socket);
        if transport.answers() {
            return Ok(Some(transport));
        }
    }
    let not_running = |message: String| CliError::new(Exit::NotRunning, message);
    match crate::reload_signal::lock_holder(lock_path) {
        LockHolder::Free => Ok(None),
        LockHolder::Held(pid) => match read().control_socket_unavailable {
            Some(reason) => Err(not_running(format!(
                "dux (PID {pid}) is running without a control socket: {reason}"
            ))),
            None => Err(not_running(format!(
                "dux (PID {pid}) is running but does not answer on its control socket; restart it"
            ))),
        },
        LockHolder::Unknown(reason) => Err(not_running(format!(
            "could not tell whether dux is running: {reason}"
        ))),
    }
}

impl Client {
    /// The dux this client talks to, as a prompt or a message names it.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The saved remote's name, when the target is one.
    pub fn remote_name(&self) -> Option<&str> {
        self.remote.as_deref()
    }

    /// Send `request` with this client's sign-in. No reply is
    /// [`Exit::NotRunning`]; a reply of any status is returned.
    pub fn send(&self, mut request: Request) -> Result<Response, CliError> {
        request.bearer = self.token.clone();
        self.transport
            .send(&request)
            .map_err(|error| self.no_reply(&error))
    }

    /// [`Self::send`] without mapping a missing reply, for a caller that
    /// retries one.
    pub fn try_send(&self, mut request: Request) -> Result<Response, TransportError> {
        request.bearer = self.token.clone();
        self.transport.send(&request)
    }

    /// The error for a request that got no reply.
    pub fn no_reply(&self, error: &TransportError) -> CliError {
        match error {
            TransportError::Refused(why) => CliError::new(Exit::Failed, why.clone()),
            _ => CliError::new(
                Exit::NotRunning,
                format!("{} does not answer: {error}", self.target),
            ),
        }
    }

    /// GET `path` and read its JSON; any status but 200 is the dux's refusal.
    pub fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, CliError> {
        let reply = self.send(Request::get(path))?;
        if reply.status != 200 {
            return Err(self.refusal(&reply));
        }
        self.parse(&reply)
    }

    /// `reply`'s body as `T`.
    pub fn parse<T: DeserializeOwned>(&self, reply: &Response) -> Result<T, CliError> {
        serde_json::from_slice(&reply.body).map_err(|error| {
            CliError::new(
                Exit::Failed,
                format!(
                    "{} answered something this client cannot read: {error}",
                    self.target
                ),
            )
        })
    }

    /// The error a non-success reply means: the dux's own sentence, and the
    /// exit code its status calls for.
    pub fn refusal(&self, reply: &Response) -> CliError {
        if reply.status == 401
            && let Some(name) = &self.remote
        {
            return login_needed(name);
        }
        let exit = match reply.status {
            409 => Exit::Refused,
            503 => Exit::NotRunning,
            _ => Exit::Failed,
        };
        CliError::new(exit, reply_sentence(reply))
    }

    /// A remote that asks this client for its password needs a sign-in;
    /// one that does not is used with none.
    fn check_sign_in(&self) -> Result<(), CliError> {
        #[derive(serde::Deserialize)]
        struct AuthStatus {
            required_here: bool,
        }
        let status: AuthStatus = self.get_json("/api/v1/auth/status")?;
        match (&self.remote, status.required_here, &self.token) {
            (Some(name), true, None) => Err(login_needed(name)),
            _ => Ok(()),
        }
    }

    /// The dux must speak this client's API version.
    fn check_api(&self) -> Result<(), CliError> {
        #[derive(serde::Deserialize)]
        struct Build {
            version: String,
            #[serde(default)]
            api: Option<u64>,
        }
        let build: Build = self.get_json("/api/v1/build")?;
        if build.api == Some(API_VERSION) {
            return Ok(());
        }
        let theirs = match build.api {
            Some(api) => format!("API version {api}"),
            None => "a different API version".to_string(),
        };
        Err(CliError::new(
            Exit::Failed,
            format!(
                "{} runs dux {}, which speaks {theirs}, and this command line is dux {}, which \
                 speaks API version {API_VERSION}; run the same dux version on both",
                self.target,
                build.version,
                crate::display_version()
            ),
        ))
    }
}

fn login_needed(name: &str) -> CliError {
    CliError::new(
        Exit::PasswordNeeded,
        format!("{name} asks for its password; run \"dux remote login {name}\""),
    )
}

/// The sentence a refusal carries: a JSON body's `message` (else its
/// `error`), a plain-text body as it is, or the status alone.
pub fn reply_sentence(reply: &Response) -> String {
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&reply.body) {
        for field in ["message", "error"] {
            if let Some(text) = json.get(field).and_then(|v| v.as_str()) {
                return text.to_string();
            }
        }
    }
    let text = String::from_utf8_lossy(&reply.body).trim().to_string();
    if text.is_empty() {
        format!("dux answered with HTTP status {}", reply.status)
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    const BUILD: &str = r#"{"version":"v1.2.3","process":"p","api":1}"#;

    /// A stand-in local dux: a socket answering `/api/v1/build`, named in a
    /// lock file held by this process.
    fn local_dux(dir: &Path) -> (FakeDux, crate::lockfile::SingleInstanceLock) {
        let socket = dir.join("dux.sock");
        let fake = FakeDux::unix(&socket, |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            _ => Reply::json(404, "{}"),
        });
        let lock = crate::lockfile::SingleInstanceLock::acquire(&dir.join("dux.lock")).unwrap();
        let text = std::fs::read_to_string(dir.join("dux.lock")).unwrap();
        std::fs::write(
            dir.join("dux.lock"),
            format!("{text}control-socket={}\n", socket.display()),
        )
        .unwrap();
        (fake, lock)
    }

    #[test]
    fn the_target_is_local_then_remote_then_variable_then_default() {
        assert_eq!(
            selected_remote(Some("a"), false, Some("b"), Some("c")).as_deref(),
            Some("a")
        );
        assert_eq!(
            selected_remote(None, false, Some("b"), Some("c")).as_deref(),
            Some("b")
        );
        assert_eq!(
            selected_remote(None, false, Some(""), Some("c")).as_deref(),
            Some("c")
        );
        assert_eq!(selected_remote(None, false, None, None), None);
        assert_eq!(selected_remote(None, true, Some("b"), Some("c")), None);

        let unknown = choose_target(Some("nowhere"), false, None, &Remotes::default()).unwrap_err();
        assert_eq!(unknown.exit, Exit::Usage);
        assert!(unknown.message.contains("nowhere"));
    }

    #[test]
    fn with_the_lock_free_dux_is_not_running_and_the_database_is_never_created() {
        let dir = private_dir();
        let error = connect(&Target::Local, &dir.path().join("dux.lock")).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert_eq!(
            error.message,
            "dux isn't running; start it with \"dux\" or \"dux server\""
        );
        // A lock file left by a dux that exited, naming a socket nobody serves.
        std::fs::write(
            dir.path().join("dux.lock"),
            format!(
                "999999\ncontrol-socket={}\n",
                dir.path().join("gone.sock").display()
            ),
        )
        .unwrap();
        let error = connect(&Target::Local, &dir.path().join("dux.lock")).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert!(
            error.message.starts_with("dux isn't running"),
            "{}",
            error.message
        );
        assert!(!dir.path().join("sessions.sqlite3").exists());
    }

    #[test]
    fn a_held_lock_with_no_socket_says_why() {
        let dir = private_dir();
        let lock_path = dir.path().join("dux.lock");
        let mut lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        let pid = std::process::id();

        let error = connect(&Target::Local, &lock_path).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert_eq!(
            error.message,
            format!(
                "dux (PID {pid}) is running but does not answer on its control socket; restart it"
            )
        );

        let too_long = dir.path().join("x".repeat(120)).join("dux.sock");
        let _ = lock.open_control_socket(&too_long);
        let error = connect(&Target::Local, &lock_path).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert!(
            error.message.starts_with(&format!(
                "dux (PID {pid}) is running without a control socket: "
            )),
            "{}",
            error.message
        );
        assert!(
            error.message.len() > "dux (PID 1) is running without a control socket: ".len() + 3
        );
    }

    #[test]
    fn an_unreachable_remote_never_falls_back_to_the_local_dux() {
        let dir = private_dir();
        let (local, _lock) = local_dux(dir.path());
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let mut remotes = Remotes::default();
        remotes
            .add("far", &format!("http://{closed}"), false)
            .unwrap();
        remotes.set_default(Some("far")).unwrap();
        let lock_path = dir.path().join("dux.lock");

        let by_default = choose_target(None, false, None, &remotes).unwrap();
        let error = connect(&by_default, &lock_path).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert!(error.message.contains("far"), "{}", error.message);
        let error = connect(
            &choose_target(Some("far"), false, None, &remotes).unwrap(),
            &lock_path,
        )
        .unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert!(
            local.seen().is_empty(),
            "the local socket was never contacted"
        );

        let client = connect(
            &choose_target(None, true, None, &remotes).unwrap(),
            &lock_path,
        )
        .expect("--local reaches the local dux whatever the default says");
        assert_eq!(client.target(), "this machine's dux");
        assert_eq!(local.seen().len(), 1);
    }

    #[test]
    fn a_remote_that_asks_for_a_password_needs_a_sign_in_and_one_that_does_not_needs_none() {
        let serve = |required: bool| {
            FakeDux::tcp(move |seen| match seen.path.as_str() {
                "/api/v1/auth/status" => Reply::json(
                    200,
                    &format!(r#"{{"password_set":true,"required_here":{required}}}"#),
                ),
                "/api/v1/build" if required && seen.header("authorization").is_none() => {
                    Reply::json(401, r#"{"error":"auth_required"}"#)
                }
                "/api/v1/build" => Reply::json(200, BUILD),
                _ => Reply::json(404, "{}"),
            })
        };
        let lock = Path::new("/nonexistent/dux.lock");
        let remote = |addr: std::net::SocketAddr, token: Option<&str>| Target::Remote {
            name: "work".into(),
            remote: Remote {
                url: format!("http://{addr}"),
                insecure: false,
                token: token.map(str::to_string),
            },
        };

        let (_open, open_addr) = serve(false);
        connect(&remote(open_addr, None), lock).expect("no password asked, no sign-in needed");

        let (guarded, guarded_addr) = serve(true);
        let error = connect(&remote(guarded_addr, None), lock).unwrap_err();
        assert_eq!(error.exit, Exit::PasswordNeeded);
        assert_eq!(
            error.message,
            "work asks for its password; run \"dux remote login work\""
        );
        assert_eq!(guarded.seen().len(), 1, "it stops before any other request");

        connect(&remote(guarded_addr, Some("t0k")), lock).expect("a held token is sent");
        assert_eq!(
            guarded.seen().last().unwrap().header("authorization"),
            Some("Bearer t0k")
        );
    }

    #[test]
    fn a_dux_speaking_another_api_is_refused_naming_both_versions() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, |_| {
            Reply::json(200, r#"{"version":"v9.0.0","process":"p","api":2}"#)
        });
        let _lock =
            crate::lockfile::SingleInstanceLock::acquire(&dir.path().join("dux.lock")).unwrap();
        std::fs::write(
            dir.path().join("dux.lock"),
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let error = connect(&Target::Local, &dir.path().join("dux.lock")).unwrap_err();
        assert_eq!(error.exit, Exit::Failed);
        assert!(error.message.contains("v9.0.0"), "{}", error.message);
        assert!(error.message.contains("API version 2"), "{}", error.message);
        assert!(
            error.message.contains(crate::display_version()),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_refusal_carries_the_dux_sentence_and_its_exit_code() {
        let reply = |status, body: &str| Response {
            status,
            body: body.as_bytes().to_vec(),
        };
        assert_eq!(
            reply_sentence(&reply(
                409,
                r#"{"error":"changed","message":"The macro list changed."}"#
            )),
            "The macro list changed."
        );
        assert_eq!(
            reply_sentence(&reply(404, r#"{"error":"unknown_operation"}"#)),
            "unknown_operation"
        );
        assert_eq!(
            reply_sentence(&reply(400, "cannot close the only tab\n")),
            "cannot close the only tab"
        );
        assert_eq!(
            reply_sentence(&reply(500, "")),
            "dux answered with HTTP status 500"
        );
    }
}
