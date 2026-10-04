//! Signing in, using dux while signed in, signing out, and being turned away:
//! journeys 1, 2, 3 and 11 of the login plan.

use std::time::Duration;

use dux_journeys::api::{add_demo_project, create_agent, projects, session_id};
use dux_journeys::client::cookie_attributes;
use dux_journeys::util::suffix;
use dux_journeys::ws::{connect, connect_ok};
use dux_journeys::{
    Client, Dux, DuxOptions, OTHER_STRONG_PASSWORD, Response, STRONG_PASSWORD, journey,
};
use serde_json::json;

/// A protected request with no valid session: 401 `auth_required`.
pub fn assert_auth_required(response: &Response, what: &str) {
    assert_eq!(
        (response.status, response.error_code().as_deref()),
        (401, Some("auth_required")),
        "{what} must be refused with 401 auth_required: {}",
        response.describe()
    );
}

/// The session cookie dux set, as `name=value`, and its attributes.
pub fn session_cookie(response: &Response) -> (String, Vec<String>) {
    let cookies = response.set_cookies();
    assert_eq!(
        cookies.len(),
        1,
        "a login sets exactly one cookie: {:?}",
        response.headers
    );
    let raw = cookies[0];
    let pair = raw.split(';').next().unwrap_or_default().trim().to_string();
    (pair, cookie_attributes(raw))
}

/// Situation: `dux server` reachable from the network, with a password set
/// through `dux config set server.auth.password --stdin` and every other auth
/// setting at its default (`require = "network"`, so the password applies to
/// this client).
///
/// Task: a person on another machine signs in, looks at their projects, and
/// signs out.
///
/// Action: read the auth status; try to list projects; sign in with the right
/// password; list projects; sign out; list projects again; then replay the
/// cookie saved from before signing out.
///
/// Result: the status says a password is set and needed here and that this
/// client is signed out; `/healthz` stays public; listing before signing in is
/// refused with 401 `auth_required`; signing in answers 204 with one cookie that
/// is `HttpOnly`, `SameSite=Strict`, `Path=/`, carries no `Domain`, and is not
/// `Secure` on plain HTTP; the list then answers; signing out answers 204 and
/// clears the cookie; the list is refused again; and the replayed old cookie is
/// refused too, because signing out revoked the session on the server rather
/// than only asking the browser to forget it.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_01_sign_in_list_projects_sign_out_and_be_refused() {
    journey("01-sign-in-sign-out", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed().with_password(STRONG_PASSWORD)).await;
        let client = dux.client().await;

        let status = client.auth_status().await;
        assert_eq!(status["password_set"], json!(true), "{status}");
        assert_eq!(status["required_here"], json!(true), "{status}");
        assert_eq!(status["signed_in"], json!(false), "{status}");

        let health = client.get("/healthz").await;
        assert_eq!(
            health.status,
            200,
            "/healthz stays public: {}",
            health.describe()
        );

        assert_auth_required(&client.get("/api/v1/projects").await, "listing projects");

        let login = client.login(STRONG_PASSWORD).await;
        assert_eq!(login.status, 204, "{}", login.describe());
        let (cookie, attributes) = session_cookie(&login);
        assert!(
            attributes.contains(&"httponly".to_string()),
            "{attributes:?}"
        );
        assert!(
            attributes.contains(&"samesite=strict".to_string()),
            "{attributes:?}"
        );
        assert!(attributes.contains(&"path=/".to_string()), "{attributes:?}");
        assert!(
            !attributes.iter().any(|a| a.starts_with("domain")),
            "a host-only cookie: {attributes:?}"
        );
        assert!(
            !attributes.contains(&"secure".to_string()),
            "plain HTTP gets no Secure cookie: {attributes:?}"
        );
        assert_eq!(client.auth_status().await["signed_in"], json!(true));

        assert!(projects(&client).await.is_empty());

        let logout = client.logout().await;
        assert_eq!(logout.status, 204, "{}", logout.describe());
        let cleared = logout.set_cookies();
        assert!(
            cleared.iter().any(|c| {
                let lower = c.to_ascii_lowercase();
                lower.contains("max-age=0") || lower.contains("expires=thu, 01 jan 1970")
            }),
            "signing out clears the cookie: {cleared:?}"
        );
        assert_auth_required(
            &client.get("/api/v1/projects").await,
            "listing after signing out",
        );

        let replay = client.fresh();
        replay.set_cookie(&cookie);
        assert_auth_required(
            &replay.get("/api/v1/projects").await,
            "a cookie replayed after signing out",
        );
    })
    .await;
}

/// Situation: `dux server` reachable from the network, password set, the fake
/// provider configured and a demo repository on disk.
///
/// Task: a person signs in from another machine and does a whole piece of work:
/// adds a project, starts an agent, and talks to it in its terminal.
///
/// Action: try to open the events socket and an agent terminal socket before
/// signing in; sign in; add the demo repository as a project; list projects;
/// create an agent; open its terminal socket with the session cookie, claim it,
/// type `hello` and Enter; read the answer; close the socket.
///
/// Result: both sockets are refused before signing in (the upgrade answers 401);
/// after signing in every step works as it does with no password: the project
/// is listed, the agent appears, and the terminal shows `you typed hello`.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_02_signed_in_person_adds_a_project_and_talks_to_an_agent() {
    journey("02-signed-in-work", Duration::from_secs(240), async {
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_password(STRONG_PASSWORD)
                .with_env("DUX_FAKE_FIXTURE", "quit-on-command"),
        )
        .await;
        let client = dux.client().await;

        let refused = connect(&client, "/ws/events").await.err();
        assert_eq!(
            refused.as_ref().map(|r| r.status),
            Some(401),
            "the events socket needs a session: {refused:?}"
        );

        client.login_ok(STRONG_PASSWORD).await;
        let project = add_demo_project(&client).await;
        assert!(
            projects(&client)
                .await
                .iter()
                .any(|p| p["id"].as_str() == Some(project.as_str()))
        );
        let name = format!("journey-{}", suffix());
        let id = session_id(&create_agent(&client, &project, &name).await);

        let stranger = client.fresh();
        let refused = connect(&stranger, &format!("/ws/sessions/{id}/pty"))
            .await
            .err();
        assert_eq!(
            refused.as_ref().map(|r| r.status),
            Some(401),
            "a terminal socket needs a session: {refused:?}"
        );

        let mut pty = connect_ok(&client, &format!("/ws/sessions/{id}/pty")).await;
        pty.next_event("connected", Duration::from_secs(20))
            .await
            .expect("the PTY handshake");
        pty.claim(24, 80).await;
        pty.send_bytes(b"hello\r").await;
        assert!(
            pty.read_until("you typed hello", Duration::from_secs(20))
                .await,
            "the agent answers; got {:?}",
            pty.output_text()
        );
        pty.close().await;
    })
    .await;
}

/// Situation: `dux server` reachable from the network with a password and the
/// default `max_failed_logins = 5`; somebody on the network keeps guessing.
///
/// Task: dux has to stop the guessing, tell the owner exactly where the block
/// lives, and let the owner lift it; and it must never lock this machine out.
///
/// Action: from the network, sign in with a wrong password five times (waiting
/// out any `Retry-After` the slow-down asks for); read the config file; try the
/// RIGHT password, a protected request and the events socket; remove the address
/// from `blocked_addresses` by hand and send dux `SIGUSR1`, as the refusal says
/// to; sign in again. Then, from this machine, fail more times than the limit
/// and sign in with the right password.
///
/// Result: the client's address (the one dux sees: the Docker bridge gateway)
/// is appended to `[server.auth] blocked_addresses` in config.toml; from then
/// on even the right password gets 403 `blocked` with a `where` naming the
/// config file and the setting, and so does every other request and the socket
/// upgrade; once the owner removes it, the right password signs in. From this
/// machine the failures are slowed (429 with `Retry-After` is allowed) but
/// never blocked: loopback never lands in the list and the right password still
/// signs in.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_03_repeated_failures_block_the_address_until_the_owner_lifts_it() {
    journey("03-blocklist", Duration::from_secs(300), async {
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.require", "everywhere"),
        )
        .await;
        let client = dux.client().await;
        let address = dux.host_client_address().await;

        for attempt in 1..=5 {
            let response = wrong_password_until_counted(&client).await;
            assert!(
                matches!(response.status, 401 | 403),
                "attempt {attempt} should be refused as a failure: {}",
                response.describe()
            );
        }

        let config = dux.config_text().await;
        let blocked_line = config
            .lines()
            .skip_while(|l| !l.trim_start().starts_with("blocked_addresses"))
            .take_while(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            blocked_line.contains(&address),
            "the guessing address {address} is appended to blocked_addresses:\n{blocked_line}"
        );
        assert!(
            !blocked_line.contains("127.0.0.1"),
            "loopback is never blocked:\n{blocked_line}"
        );

        let right = client.login(STRONG_PASSWORD).await;
        assert_eq!(
            (right.status, right.error_code().as_deref()),
            (403, Some("blocked")),
            "a blocked address is refused even with the right password: {}",
            right.describe()
        );
        let place = right.json()["where"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            place.contains("config.toml") && place.contains("blocked_addresses"),
            "the refusal says where to lift the block: {place:?}"
        );
        let other = client.get("/api/v1/projects").await;
        assert_eq!(other.status, 403, "{}", other.describe());
        let socket = connect(&client, "/ws/events").await.err();
        assert_eq!(socket.map(|r| r.status), Some(403));

        // The owner's hand edit: empty the list, whether dux wrote it on one
        // line or across several, and leave everything else alone.
        dux.exec_ok(
            "sed -i \
               -e '/^blocked_addresses = \\[.*\\]/c\\blocked_addresses = []' \
               -e '/^blocked_addresses = \\[[^]]*$/,/\\]/c\\blocked_addresses = []' \
               \"$DUX_HOME/config.toml\"",
        )
        .await;
        assert!(
            dux.config_text().await.contains("blocked_addresses = []"),
            "the hand edit emptied the list"
        );
        dux.signal_reload().await;
        dux_journeys::eventually(
            "the lifted block to apply",
            Duration::from_secs(20),
            || async { (client.login(STRONG_PASSWORD).await.status == 204).then_some(()) },
        )
        .await;
        assert!(projects(&client).await.is_empty());

        let inside = dux.inside();
        for _ in 0..7 {
            let response = inside.login("not-the-password-at-all").await;
            assert!(
                matches!(response.status, 401 | 429),
                "this machine is slowed, never blocked: {}",
                response.describe()
            );
        }
        let config = dux.config_text().await;
        assert!(
            !config.contains("\"127.0.0.1\"") && !config.contains("\"::1\""),
            "loopback never lands in blocked_addresses"
        );
        let signed_in = dux_journeys::eventually(
            "this machine to sign in after the slow-down",
            Duration::from_secs(60),
            || async {
                let response = inside.login(STRONG_PASSWORD).await;
                assert_ne!(
                    response.status,
                    403,
                    "loopback was blocked: {}",
                    response.describe()
                );
                (response.status == 204).then_some(response)
            },
        )
        .await;
        assert_eq!(signed_in.status, 204);
    })
    .await;
}

/// One wrong-password attempt that dux counted: a 429 is the slow-down asking
/// to wait, so wait what it says and try again.
async fn wrong_password_until_counted(client: &Client) -> Response {
    loop {
        let response = client.login("definitely-not-the-password").await;
        if response.status != 429 {
            return response;
        }
        let wait: u64 = response
            .header("retry-after")
            .and_then(|v| v.parse().ok())
            .expect("a 429 carries Retry-After");
        tokio::time::sleep(Duration::from_secs(wait.max(1))).await;
    }
}

/// Situation: `dux server` with NO password yet, reachable from the network,
/// and a second `dux server` with no password on a stand-in tailnet.
///
/// Task: the owner wants to set the first password from the web, and nobody
/// else who can reach dux may do it first and lock the owner out.
///
/// Action: from the network, try to set the first password; from this machine,
/// set it; from the network, sign in with it, then try to change it with no
/// current password and with a wrong one, then change it with the right one;
/// replay the session from before the change. On the tailnet dux, set the first
/// password through the tailnet.
///
/// Result: the network attempt is refused (403) and leaves config.toml with no
/// hash, while the status says `can_set_first_password` is false there; this
/// machine's attempt answers 204, writes a `password_hash` that is an Argon2id
/// PHC string (never the password), and the status everywhere says a password
/// is set; the new password signs in from the network; a change without the
/// current password or with a wrong one is refused and changes nothing; the
/// change with the right one answers 204 and signs everyone out, so the old
/// session is refused and the new password signs in. A tailnet peer may set the
/// first password too.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_11_the_first_password_is_set_only_from_this_machine_or_the_tailnet() {
    journey("11-first-password", Duration::from_secs(300), async {
        let dux = Dux::start(DuxOptions::exposed()).await;
        let network = dux.client().await;

        let status = network.auth_status().await;
        assert_eq!(status["password_set"], json!(false), "{status}");
        assert_eq!(status["can_set_first_password"], json!(false), "{status}");
        let attempt = network
            .post_json("/api/v1/auth/password", &json!({ "new": STRONG_PASSWORD }))
            .await;
        assert_eq!(attempt.status, 403, "{}", attempt.describe());
        assert!(
            !dux.config_text().await.contains("$argon2id$"),
            "a refused first password writes nothing"
        );

        let inside = dux.inside();
        assert_eq!(
            inside.auth_status().await["can_set_first_password"],
            json!(true)
        );
        let set = inside
            .post_json("/api/v1/auth/password", &json!({ "new": STRONG_PASSWORD }))
            .await;
        assert_eq!(set.status, 204, "{}", set.describe());
        let config = dux.config_text().await;
        assert!(config.contains("password_hash = \"$argon2id$"), "{config}");
        assert!(
            !config.contains(STRONG_PASSWORD),
            "the password itself is never written"
        );
        assert_eq!(network.auth_status().await["password_set"], json!(true));

        network.login_ok(STRONG_PASSWORD).await;
        let before_change = network.cookie_header().expect("a session cookie");

        for body in [
            json!({ "new": OTHER_STRONG_PASSWORD }),
            json!({ "current": "wrong-current-password-123", "new": OTHER_STRONG_PASSWORD }),
        ] {
            let refused = network.post_json("/api/v1/auth/password", &body).await;
            assert!(
                matches!(refused.status, 401 | 403),
                "a change without the right current password is refused: {}",
                refused.describe()
            );
        }
        let changed = network
            .post_json(
                "/api/v1/auth/password",
                &json!({ "current": STRONG_PASSWORD, "new": OTHER_STRONG_PASSWORD }),
            )
            .await;
        assert_eq!(changed.status, 204, "{}", changed.describe());

        let replay = network.fresh();
        replay.set_cookie(&before_change);
        assert_auth_required(
            &replay.get("/api/v1/projects").await,
            "a session from before the change",
        );
        let again = network.fresh();
        assert_eq!(again.login(STRONG_PASSWORD).await.status, 401);
        again.login_ok(OTHER_STRONG_PASSWORD).await;

        let tailnet_dux = Dux::start(DuxOptions::local().with_tailnet(4100)).await;
        let peer = tailnet_dux.client_on(4100).await;
        crate::auth_network::wait_tailnet_leg(&peer).await;
        let set = peer
            .post_json("/api/v1/auth/password", &json!({ "new": STRONG_PASSWORD }))
            .await;
        assert_eq!(
            set.status,
            204,
            "a tailnet peer may set the first password: {}",
            set.describe()
        );
    })
    .await;
}
