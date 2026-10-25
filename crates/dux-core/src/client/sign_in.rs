//! Signing the command line in to a remote dux and out again. A sign-in is a
//! CLI token the remote issues for its password; it is kept in
//! `remotes.toml` and sent as a bearer token with every request.

use super::connect::reply_sentence;
use super::remotes::Remote;
use super::transport::{HttpTransport, Method, Request, Response, Transport, TransportError};
use super::{CliError, Exit};

/// The label a remote records for this sign-in.
const LABEL: &str = "dux CLI";

fn send(name: &str, remote: &Remote, request: &Request) -> Result<Response, CliError> {
    HttpTransport::new(&remote.url, remote.insecure)
        .send(request)
        .map_err(|error| match error {
            TransportError::Refused(why) => CliError::new(Exit::Failed, why),
            _ => CliError::new(
                Exit::NotRunning,
                format!("{name} ({}) does not answer: {error}", remote.url),
            ),
        })
}

/// What a remote says about itself before a sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteDux {
    /// Whether it has a password at all.
    pub password_set: bool,
    /// Its instance id, which a sign-in pins the remote to.
    pub instance: Option<String>,
}

/// Whether the remote has a password at all, and which dux it is.
pub fn look(name: &str, remote: &Remote) -> Result<RemoteDux, CliError> {
    let reply = send(name, remote, &Request::get("/api/v1/auth/status"))?;
    let status = super::connect::read_auth_status(&format!("{name} ({})", remote.url), &reply)?;
    Ok(RemoteDux {
        password_set: status.password_set,
        instance: status.instance,
    })
}

/// The instance id to keep with a sign-in that just succeeded: the remote is
/// asked again, and a dux other than the one `before` found when the sign-in
/// began refuses the sign-in rather than pin the remote to either.
pub fn pin_after_login(
    name: &str,
    remote: &Remote,
    before: &RemoteDux,
) -> Result<Option<String>, CliError> {
    let after = look(name, remote)?;
    if let (Some(was), Some(now)) = (&before.instance, &after.instance)
        && was != now
    {
        return Err(CliError::new(
            Exit::Failed,
            format!(
                "{name} ({}) became a different dux during the sign-in, so the sign-in was not \
                 kept; run \"dux remote login {name}\" again",
                remote.url
            ),
        ));
    }
    Ok(after.instance)
}

/// A sign-in the remote issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedIn {
    /// The remote's CLI token.
    pub token: String,
    /// Why the sign-in this one replaces could not be ended, when it could not.
    pub old_sign_in_kept: Option<String>,
}

/// Sign in with `password` and return the remote's CLI token.
pub fn login(name: &str, remote: &Remote, password: &str) -> Result<SignedIn, CliError> {
    let body = serde_json::json!({ "password": password, "label": LABEL });
    let reply = send(
        name,
        remote,
        &Request {
            method: Method::Post,
            path: "/api/v1/auth/cli-login".to_string(),
            body: Some(body.to_string().into_bytes()),
            bearer: None,
            timeout: None,
        },
    )?;
    if reply.status != 200 {
        return Err(CliError::new(Exit::Failed, reply_sentence(&reply)));
    }
    #[derive(serde::Deserialize)]
    struct Issued {
        token: String,
    }
    let token = serde_json::from_slice::<Issued>(&reply.body)
        .map(|issued| issued.token)
        .map_err(|error| {
            CliError::new(
                Exit::Failed,
                format!(
                    "{name} answered the sign-in with something this client cannot read: {error}"
                ),
            )
        })?;
    // The sign-in this one replaces is ended, as a logout would, so a re-login
    // never leaves a token alive that nothing here holds any more.
    // One the remote answers 401 for has ended already (a password change ends them all).
    let old_sign_in_kept = remote.token.as_deref().and_then(|old| {
        let ended = token_may_go(name, remote).and_then(|()| end_sign_in(name, remote, old));
        let why = match ended {
            Ok(reply) if (200..300).contains(&reply.status) || reply.status == 401 => {
                return None;
            }
            Ok(reply) => reply_sentence(&reply),
            Err(error) => error.message,
        };
        Some(format!(
            "Signed in again, but could not tell {name} to end the previous sign-in: {}. It \
             ends on its own once it goes unused for [server.auth] cli_token_idle_days.",
            why.trim_end_matches('.')
        ))
    });
    Ok(SignedIn {
        token,
        old_sign_in_kept,
    })
}

/// Whether a saved sign-in may be sent to what answers at `remote` now: only
/// to the dux it was issued by. The remote is asked without it; a remote saved
/// before dux had an id was never pinned, and is sent it as before.
fn token_may_go(name: &str, remote: &Remote) -> Result<(), CliError> {
    let Some(pinned) = &remote.instance else {
        return Ok(());
    };
    let now = look(name, remote)?;
    let target = format!("{name} ({})", remote.url);
    match now.instance {
        Some(id) if &id == pinned => Ok(()),
        Some(_) => Err(CliError::new(
            Exit::Failed,
            format!("{target} is now a different dux"),
        )),
        None => Err(CliError::new(
            Exit::Failed,
            format!("{target} does not say which dux it is"),
        )),
    }
}

/// Ask the remote to end the sign-in `token`, once it is the dux that issued it.
pub fn logout(name: &str, remote: &Remote, token: &str) -> Result<(), CliError> {
    token_may_go(name, remote)?;
    let reply = end_sign_in(name, remote, token)?;
    if (200..300).contains(&reply.status) {
        Ok(())
    } else {
        Err(CliError::new(Exit::Failed, reply_sentence(&reply)))
    }
}

/// Send the request that ends the sign-in `token`, and hand back the reply.
fn end_sign_in(name: &str, remote: &Remote, token: &str) -> Result<Response, CliError> {
    send(
        name,
        remote,
        &Request {
            method: Method::Post,
            path: "/api/v1/auth/cli-logout".to_string(),
            body: None,
            bearer: Some(token.to_string()),
            timeout: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::test_server::{FakeDux, Reply};

    #[test]
    fn a_sign_in_returns_the_token_ends_the_one_it_replaces_and_a_wrong_password_gets_the_remote_sentence()
     {
        let (fake, addr) = FakeDux::tcp(|seen| match seen.path.as_str() {
            "/api/v1/auth/status" => Reply::json(
                200,
                r#"{"password_set":true,"required_here":true,"signed_in":false}"#,
            ),
            "/api/v1/auth/cli-login" if seen.body.contains(r#""password":"right""#) => {
                Reply::json(200, r#"{"token":"T0K","expires_at":null}"#)
            }
            "/api/v1/auth/cli-login" => Reply::json(
                401,
                r#"{"error":"wrong_password","message":"That password is not right."}"#,
            ),
            "/api/v1/auth/cli-logout" if seen.header("authorization") == Some("Bearer BAD") => {
                Reply::json(500, r#"{"message":"the session store is busy"}"#)
            }
            "/api/v1/auth/cli-logout" if seen.header("authorization") == Some("Bearer GONE") => {
                Reply::json(401, r#"{"error":"auth_required"}"#)
            }
            "/api/v1/auth/cli-logout" => Reply::Raw("HTTP/1.1 204 No Content\r\n\r\n".into()),
            _ => Reply::json(404, "{}"),
        });
        let remote = Remote {
            url: format!("http://{addr}"),
            insecure: false,
            token: None,
            instance: None,
        };
        assert!(look("work", &remote).unwrap().password_set);

        let wrong = login("work", &remote, "nope").unwrap_err();
        assert_eq!(wrong.exit, Exit::Failed);
        assert_eq!(wrong.message, "That password is not right.");

        let signed_in = login("work", &remote, "right").unwrap();
        assert_eq!(signed_in.token, "T0K");
        assert_eq!(signed_in.old_sign_in_kept, None);
        let sign_in = fake
            .seen()
            .into_iter()
            .rfind(|s| s.path == "/api/v1/auth/cli-login")
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&sign_in.body).unwrap();
        assert_eq!(body["label"], "dux CLI");

        logout("work", &remote, "T0K").unwrap();
        assert_eq!(
            fake.seen().last().unwrap().header("authorization"),
            Some("Bearer T0K")
        );

        // Signing in again ends the sign-in it replaces, once the new one is issued.
        let again = Remote {
            token: Some("OLD".to_string()),
            ..remote.clone()
        };
        let signed_in = login("work", &again, "right").unwrap();
        assert_eq!(signed_in.token, "T0K");
        assert_eq!(signed_in.old_sign_in_kept, None);
        let last = fake.seen().last().unwrap().clone();
        assert_eq!(last.path, "/api/v1/auth/cli-logout");
        assert_eq!(last.header("authorization"), Some("Bearer OLD"));
        // An old sign-in the remote would not end is said, and the new one still comes back.
        let stuck = Remote {
            token: Some("BAD".to_string()),
            ..remote.clone()
        };
        let signed_in = login("work", &stuck, "right").unwrap();
        assert_eq!(signed_in.token, "T0K");
        let kept = signed_in.old_sign_in_kept.expect("a warning");
        assert!(kept.contains("the session store is busy"), "{kept}");
        assert!(kept.contains("cli_token_idle_days"), "{kept}");
        // One the remote no longer knows (a password change ended it) has nothing to end.
        let gone = Remote {
            token: Some("GONE".to_string()),
            ..remote.clone()
        };
        assert_eq!(
            login("work", &gone, "right").unwrap().old_sign_in_kept,
            None
        );
        // A wrong password leaves the old sign-in alone.
        let before = fake.seen().len();
        assert!(login("work", &again, "nope").is_err());
        assert!(
            fake.seen()[before..]
                .iter()
                .all(|seen| seen.path != "/api/v1/auth/cli-logout")
        );
    }

    #[test]
    fn the_sign_in_keeps_the_dux_that_answered_it_and_refuses_one_that_changed_during_it() {
        let instance = std::sync::Arc::new(std::sync::Mutex::new("a"));
        let answers = std::sync::Arc::clone(&instance);
        let (_fake, addr) = FakeDux::tcp(move |seen| match seen.path.as_str() {
            "/api/v1/auth/status" => Reply::json(
                200,
                &format!(
                    r#"{{"password_set":true,"required_here":true,"signed_in":false,"instance":"{}"}}"#,
                    answers.lock().unwrap()
                ),
            ),
            _ => Reply::json(404, "{}"),
        });
        let remote = Remote {
            url: format!("http://{addr}"),
            insecure: false,
            token: None,
            instance: None,
        };
        let before = look("work", &remote).unwrap();
        assert_eq!(
            pin_after_login("work", &remote, &before)
                .unwrap()
                .as_deref(),
            Some("a")
        );
        *instance.lock().unwrap() = "b";
        let error = pin_after_login("work", &remote, &before).unwrap_err();
        assert_eq!(
            error.message,
            format!(
                "work (http://{addr}) became a different dux during the sign-in, so the sign-in \
                 was not kept; run \"dux remote login work\" again"
            )
        );
    }

    #[test]
    fn a_saved_sign_in_is_sent_only_to_the_dux_that_issued_it() {
        let (fake, addr) = FakeDux::tcp(|seen| match seen.path.as_str() {
            "/api/v1/auth/status" => Reply::json(
                200,
                r#"{"password_set":true,"required_here":true,"signed_in":false,"instance":"b"}"#,
            ),
            "/api/v1/auth/cli-login" => Reply::json(200, r#"{"token":"NEW","expires_at":null}"#),
            "/api/v1/auth/cli-logout" => Reply::Raw("HTTP/1.1 204 No Content\r\n\r\n".into()),
            _ => Reply::json(404, "{}"),
        });
        let pinned_elsewhere = Remote {
            url: format!("http://{addr}"),
            insecure: false,
            token: Some("OLD".to_string()),
            instance: Some("a".to_string()),
        };
        let error = logout("work", &pinned_elsewhere, "OLD").unwrap_err();
        assert_eq!(
            error.message,
            format!("work (http://{addr}) is now a different dux")
        );
        let signed_in = login("work", &pinned_elsewhere, "right").unwrap();
        assert_eq!(
            signed_in.old_sign_in_kept.as_deref(),
            Some(
                format!(
                    "Signed in again, but could not tell work to end the previous sign-in: work \
                     (http://{addr}) is now a different dux. It ends on its own once it goes \
                     unused for [server.auth] cli_token_idle_days."
                )
                .as_str()
            )
        );
        assert!(
            fake.seen()
                .iter()
                .all(|seen| seen.header("authorization") != Some("Bearer OLD")),
            "the old sign-in never went to the other dux"
        );

        // The dux that issued it is told, as before.
        let same = Remote {
            instance: Some("b".to_string()),
            ..pinned_elsewhere
        };
        logout("work", &same, "OLD").unwrap();
        assert_eq!(
            fake.seen().last().unwrap().header("authorization"),
            Some("Bearer OLD")
        );
    }

    #[test]
    fn a_gateway_page_is_a_dux_that_does_not_answer_yet() {
        let (_fake, addr) = FakeDux::tcp(|_| {
            Reply::Raw(
                "HTTP/1.1 503 X\r\ncontent-type: text/html\r\ncontent-length: 6\r\n\
                 connection: close\r\n\r\n<html>"
                    .into(),
            )
        });
        let remote = Remote {
            url: format!("http://{addr}"),
            insecure: false,
            token: None,
            instance: None,
        };
        let error = look("work", &remote).unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert_eq!(
            error.message,
            format!("work (http://{addr}) does not answer: HTTP status 503")
        );
    }

    #[test]
    fn a_remote_that_does_not_answer_is_named() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let remote = Remote {
            url: format!("http://{closed}"),
            insecure: false,
            token: None,
            instance: None,
        };
        let error = login("work", &remote, "x").unwrap_err();
        assert_eq!(error.exit, Exit::NotRunning);
        assert!(
            error.message.starts_with("work (http://"),
            "{}",
            error.message
        );
    }
}
