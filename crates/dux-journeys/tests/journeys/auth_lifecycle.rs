//! How long a session lives, what keeps it alive, and what ends it: journeys 4
//! and 5 of the login plan, and the live revocation of open sockets.

use std::time::Duration;

use dux_journeys::api::{add_demo_project, create_agent, session_id};
use dux_journeys::util::suffix;
use dux_journeys::ws::{CLOSE_SIGNED_OUT, Ended, assert_accepted_then_closed, connect_ok};
use dux_journeys::{Dux, DuxOptions, OTHER_STRONG_PASSWORD, STRONG_PASSWORD, journey};

use crate::auth_login::{assert_auth_required, assert_network_class};

/// Situation: `dux server` reachable from the network, password set, and
/// `session_idle_seconds` lowered to 3 so a test can outwait it.
///
/// Task: a person leaves a tab open while an agent works and comes back long
/// after the idle timeout; they must still be signed in. Once they close the
/// tab, the session must expire on its own.
///
/// Action: sign in; open the events socket the way an open tab does and keep it
/// open (answering dux's pings) for well over the idle timeout, making no other
/// request; then list projects. Close the socket, wait well over the idle
/// timeout again, and list projects.
///
/// Result: with the socket open the list answers after the long quiet stretch,
/// because the open socket counts as activity; with it closed the session
/// expires and the list is refused with 401 `auth_required`.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_04_an_open_socket_keeps_the_session_alive_until_it_closes() {
    journey("04-idle-session", Duration::from_secs(180), async {
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.session_idle_seconds", "3"),
        )
        .await;
        let client = dux.client().await;
        assert_network_class(&client).await;
        client.login_ok(STRONG_PASSWORD).await;

        let mut tab = connect_ok(&client, "/ws/events").await;
        assert_eq!(
            tab.hold_open(Duration::from_secs(10)).await,
            None,
            "the open tab's socket stays open while it is signed in"
        );
        let kept = client.get("/api/v1/projects").await;
        assert_eq!(
            kept.status,
            200,
            "an open socket keeps the session alive past the idle timeout: {}",
            kept.describe()
        );

        tab.close().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_auth_required(
            &client.get("/api/v1/projects").await,
            "a session idle past its timeout with no socket open",
        );
    })
    .await;
}

/// Situation: `dux server` reachable from the network, password set,
/// `session_idle_seconds = 8`, sessions kept in SQLite, and a restart loop so
/// the dux process can be stopped and come back behind the same port.
///
/// Task: dux restarts quickly (an upgrade, a crash) while a person has a tab
/// open; the tab must get straight back in. A restart that keeps dux down past
/// the idle timeout must not.
///
/// Action: sign in and open the events socket; restart dux; reopen the socket
/// with the same cookie and list projects, without signing in again. Then
/// stop dux for well over the idle timeout with no socket open, let it come
/// back, and list projects with the same cookie.
///
/// Result: after the quick restart the old socket ends, the reopened one
/// upgrades and the list answers with the session from before the restart.
/// After the long outage the same cookie is refused with 401 `auth_required`.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_05_a_quick_restart_lets_an_open_tab_straight_back_in() {
    journey("05-restart", Duration::from_secs(240), async {
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_restart_loop()
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.session_idle_seconds", "8"),
        )
        .await;
        let client = dux.client().await;
        assert_network_class(&client).await;
        client.login_ok(STRONG_PASSWORD).await;
        let mut tab = connect_ok(&client, "/ws/events").await;
        assert_eq!(tab.hold_open(Duration::from_secs(1)).await, None);

        dux.restart_process().await;
        assert!(
            tab.wait_ended(Duration::from_secs(10)).await.is_some(),
            "the old run's socket ends with it"
        );
        let reopened = connect_ok(&client, "/ws/events").await;
        let back = client.get("/api/v1/projects").await;
        assert_eq!(
            back.status,
            200,
            "the session survives a quick restart: {}",
            back.describe()
        );
        reopened.close().await;

        dux.stop_process_for(Duration::from_secs(14)).await;
        assert_auth_required(
            &client.get("/api/v1/projects").await,
            "a session whose idle window ran out while dux was down",
        );
    })
    .await;
}

/// Situation: `dux server` reachable from the network, password set, an agent
/// running on the fake provider, and two people (two cookie jars, A and B) each
/// signed in with the events socket and the agent's terminal socket open.
///
/// Task: signing out, and changing the password, must cut off what is already
/// connected, not only what connects next.
///
/// Action: A signs out. Then the owner changes the password with
/// `dux config set server.auth.password --stdin`.
///
/// Result: A's two sockets close with code 4401 within a few seconds while B's
/// stay open; after the password change B's two sockets close with 4401 too,
/// B's cookie is refused with 401 `auth_required`, and B's terminal socket
/// stops carrying output. The new password signs in.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_signing_out_or_changing_the_password_closes_open_sockets() {
    journey("revocation", Duration::from_secs(300), async {
        let dux = Dux::start(DuxOptions::exposed().with_password(STRONG_PASSWORD)).await;
        let a = dux.client().await;
        assert_network_class(&a).await;
        let b = a.fresh();
        a.login_ok(STRONG_PASSWORD).await;
        b.login_ok(STRONG_PASSWORD).await;

        let project = add_demo_project(&a).await;
        let id = session_id(&create_agent(&a, &project, &format!("revoke-{}", suffix())).await);
        let pty_path = format!("/ws/sessions/{id}/pty");

        let mut a_events = connect_ok(&a, "/ws/events").await;
        let mut a_pty = connect_ok(&a, &pty_path).await;
        let mut b_events = connect_ok(&b, "/ws/events").await;
        let mut b_pty = connect_ok(&b, &pty_path).await;

        assert_eq!(a.logout().await.status, 204);
        assert_eq!(
            a_events.wait_ended(Duration::from_secs(10)).await,
            Some(Ended::Closed(CLOSE_SIGNED_OUT)),
            "signing out closes that person's events socket"
        );
        assert_eq!(
            a_pty.wait_ended(Duration::from_secs(10)).await,
            Some(Ended::Closed(CLOSE_SIGNED_OUT)),
            "signing out closes that person's terminal socket"
        );
        assert_eq!(
            b_events.hold_open(Duration::from_secs(2)).await,
            None,
            "B is untouched"
        );
        assert_eq!(
            b_pty.hold_open(Duration::from_secs(1)).await,
            None,
            "B is untouched"
        );

        let changed = dux
            .config_set_secret("server.auth.password", OTHER_STRONG_PASSWORD)
            .await;
        assert_eq!(changed.code, 0, "{}", changed.output());
        assert_eq!(
            b_events.wait_ended(Duration::from_secs(15)).await,
            Some(Ended::Closed(CLOSE_SIGNED_OUT)),
            "a password change signs everyone out, open sockets included"
        );
        assert_eq!(
            b_pty.wait_ended(Duration::from_secs(10)).await,
            Some(Ended::Closed(CLOSE_SIGNED_OUT))
        );
        assert_auth_required(
            &b.get("/api/v1/projects").await,
            "a session from before the change",
        );
        assert_accepted_then_closed(
            &b,
            "/ws/events",
            CLOSE_SIGNED_OUT,
            "a socket opened with a session from before the change",
        )
        .await;
        b.fresh().login_ok(OTHER_STRONG_PASSWORD).await;
    })
    .await;
}
