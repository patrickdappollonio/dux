//! Signing the command line in to a remote dux and out again. A sign-in is a
//! CLI token the remote issues for its password; it is kept in
//! `remotes.toml` and sent as a bearer token with every request.

use super::connect::reply_sentence;
use super::remotes::Remote;
use super::transport::{HttpTransport, Method, Request, Response, Transport, TransportError};
use super::{CliError, Exit};

/// The label a remote records for this sign-in.
const LABEL: &str = "dux CLI";

fn send(name: &str, remote: &Remote, request: Request) -> Result<Response, CliError> {
    HttpTransport::new(&remote.url, remote.insecure)
        .send(&request)
        .map_err(|error| match error {
            TransportError::Refused(why) => CliError::new(Exit::Failed, why),
            _ => CliError::new(
                Exit::NotRunning,
                format!("{name} ({}) does not answer: {error}", remote.url),
            ),
        })
}

/// Whether the remote has a password at all.
pub fn password_set(name: &str, remote: &Remote) -> Result<bool, CliError> {
    #[derive(serde::Deserialize)]
    struct AuthStatus {
        password_set: bool,
    }
    let reply = send(name, remote, Request::get("/api/v1/auth/status"))?;
    if reply.status != 200 {
        return Err(CliError::new(Exit::Failed, reply_sentence(&reply)));
    }
    serde_json::from_slice::<AuthStatus>(&reply.body)
        .map(|status| status.password_set)
        .map_err(|error| {
            CliError::new(
                Exit::Failed,
                format!("{name} answered something this client cannot read: {error}"),
            )
        })
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
        Request {
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
        let why = match end_sign_in(name, remote, old) {
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

/// Ask the remote to end the sign-in `token`.
pub fn logout(name: &str, remote: &Remote, token: &str) -> Result<(), CliError> {
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
        Request {
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
            "/api/v1/auth/status" => {
                Reply::json(200, r#"{"password_set":true,"required_here":true}"#)
            }
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
        };
        assert!(password_set("work", &remote).unwrap());

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
    fn a_remote_that_does_not_answer_is_named() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let remote = Remote {
            url: format!("http://{closed}"),
            insecure: false,
            token: None,
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
