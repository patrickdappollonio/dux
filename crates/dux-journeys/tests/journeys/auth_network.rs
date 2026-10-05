//! Who has to sign in depends on where the request comes from: journeys 6 and
//! 9 of the login plan, and the headerless forward onto loopback.

use std::time::Duration;

use dux_journeys::{Dux, DuxOptions, STRONG_PASSWORD, eventually, journey};
use serde_json::json;

use crate::auth_login::assert_auth_required;

/// Set `server.auth.require` through the CLI, which signals the running dux.
async fn set_require(dux: &Dux, value: &str) {
    let run = dux.config_set("server.auth.require", value).await;
    assert_eq!(
        run.code,
        0,
        "dux config set server.auth.require {value}: {}",
        run.output()
    );
}

/// Situation: a password is set. One dux listens the way an unconfigured dux
/// does (loopback plus the stand-in Tailscale leg) and another listens on every
/// interface, so there is a client of each class: this machine, a tailnet peer,
/// and a client from the network.
///
/// Task: the owner chooses with `server.auth.require` who has to sign in, and
/// each choice must mean exactly what the config comment says.
///
/// Action: for `network`, `tailnet` and `everywhere` in turn (set live with
/// `dux config set`), list projects with no session from this machine, from the
/// tailnet peer, and from the network, and read each one's auth status.
///
/// Result:
///
/// | require    | this machine | tailnet peer | network |
/// |------------|--------------|--------------|---------|
/// | network    | allowed      | allowed      | 401     |
/// | tailnet    | allowed      | 401          | 401     |
/// | everywhere | 401          | 401          | 401     |
///
/// and each client's `required_here` agrees with its row. The tailnet peer's
/// status also reports its transport as encrypted, and the network client's as
/// not.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_06_require_decides_who_has_to_sign_in() {
    journey("06-require", Duration::from_secs(300), async {
        let local = Dux::start(
            DuxOptions::local()
                .with_tailnet(4100)
                .with_password(STRONG_PASSWORD),
        )
        .await;
        let exposed = Dux::start(DuxOptions::exposed().with_password(STRONG_PASSWORD)).await;
        let peer = local.client_on(4100).await;
        peer.wait_answering().await;
        let network = exposed.client().await;

        let tailnet_status = peer.auth_status().await;
        assert_eq!(
            tailnet_status["transport_encrypted"],
            json!(true),
            "{tailnet_status}"
        );
        let network_status = network.auth_status().await;
        assert_eq!(
            network_status["transport_encrypted"],
            json!(false),
            "{network_status}"
        );

        for (require, machine_allowed, tailnet_allowed) in [
            ("network", true, true),
            ("tailnet", true, false),
            ("everywhere", false, false),
        ] {
            set_require(&local, require).await;
            set_require(&exposed, require).await;
            // The setting reaches the running dux through a signal; its own
            // status saying so is the moment to look.
            eventually(
                &format!("require = {require} to reach the running dux"),
                Duration::from_secs(20),
                || async {
                    let machine = local.inside().auth_status().await["required_here"].clone();
                    let tailnet = peer.auth_status().await["required_here"].clone();
                    (machine == json!(!machine_allowed) && tailnet == json!(!tailnet_allowed))
                        .then_some(())
                },
            )
            .await;

            let machine = local.inside().get("/api/v1/projects").await;
            let tailnet = peer.get("/api/v1/projects").await;
            let far = network.get("/api/v1/projects").await;
            check(require, "this machine", machine_allowed, &machine);
            check(require, "a tailnet peer", tailnet_allowed, &tailnet);
            check(require, "the network", false, &far);

            assert_eq!(
                local.inside().auth_status().await["required_here"],
                json!(!machine_allowed),
                "require = {require}: this machine's required_here"
            );
            assert_eq!(
                peer.auth_status().await["required_here"],
                json!(!tailnet_allowed),
                "require = {require}: the tailnet peer's required_here"
            );
            assert_eq!(
                network.auth_status().await["required_here"],
                json!(true),
                "require = {require}: the network client's required_here"
            );
        }
    })
    .await;
}

fn check(require: &str, who: &str, allowed: bool, response: &dux_journeys::Response) {
    if allowed {
        assert_eq!(
            response.status,
            200,
            "require = {require}: {who} needs no password: {}",
            response.describe()
        );
    } else {
        assert_auth_required(
            response,
            &format!("require = {require}: {who} without a session"),
        );
    }
}

/// Situation: dux listens on loopback with a password and the default
/// `require = "network"`; the stand-in Tailscale reports no forward yet; a port
/// relays raw TCP onto loopback with no forwarding header, which is what a
/// `tailscale serve` TCP forward (or any port forwarder) looks like to dux.
///
/// Task: a headerless forward can carry anybody, so once dux can see one
/// pointing at its port it must stop trusting loopback as "this machine".
///
/// Action: switch the stand-in's serve configuration to a TCP forward onto
/// dux's port; wait out dux's next look; list projects with no session through
/// the relay and from this machine; then sign in through the relay.
///
/// Result: with the forward visible both requests are refused with 401
/// `auth_required` (loopback without a trustworthy origin counts as the
/// network), and signing in through the relay works.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_a_headerless_forward_onto_loopback_is_not_this_machine() {
    journey("headerless-forward", Duration::from_secs(240), async {
        let dux = Dux::start(
            DuxOptions::local()
                .with_tailnet(4100)
                .with_loopback_relay(4200)
                .with_password(STRONG_PASSWORD),
        )
        .await;
        dux.client_on(4100).await.wait_answering().await;
        let relayed = dux.client_on(4200).await;
        // Until its first look at Tailscale lands dux trusts no loopback
        // request, so the starting point is the moment it trusts this machine.
        eventually(
            "dux to take this machine for this machine before any forward exists",
            Duration::from_secs(30),
            || async { (dux.inside().get("/api/v1/projects").await.status == 200).then_some(()) },
        )
        .await;

        dux.set_fake_serve(&format!(
            r#"{{"TCP":{{"443":{{"TCPForward":"127.0.0.1:{}"}}}}}}"#,
            dux_journeys::DUX_PORT
        ))
        .await;
        eventually(
            "dux to see the TCP forward and stop trusting loopback",
            Duration::from_secs(30),
            || async { (relayed.get("/api/v1/projects").await.status == 401).then_some(()) },
        )
        .await;
        assert_auth_required(
            &relayed.get("/api/v1/projects").await,
            "a request through the headerless forward",
        );
        assert_auth_required(
            &dux.inside().get("/api/v1/projects").await,
            "a loopback request once a forward to dux exists",
        );
        relayed.login_ok(STRONG_PASSWORD).await;
        assert_eq!(relayed.get("/api/v1/projects").await.status, 200);
    })
    .await;
}

/// Situation: `dux server` listening on every interface with NO password, so
/// anyone who can reach it from the network has everything.
///
/// Task: dux has to warn the owner loudly, in the terminal and in the web, and
/// let them silence the web warning for good.
///
/// Action: read what dux printed at start; read the auth status from the
/// network; post "don't show again" (`dismiss-no-auth-warning`); read the
/// config file and the status again.
///
/// Result: the start output carries a warning that names the missing password;
/// the status says no password is set and `no_auth_warning` is true; the
/// dismissal answers 204 and writes `disable_no_auth_warning = true` into
/// config.toml (keeping its comment), after which `no_auth_warning` is false.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_09_no_password_while_exposed_warns_until_told_never_again() {
    journey("09-no-password-warning", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed()).await;
        let client = dux.client().await;

        let printed = dux.logs().to_ascii_lowercase();
        assert!(
            printed
                .lines()
                .any(|l| l.contains("password")
                    && (l.contains("warning") || l.contains("no password"))),
            "dux warns at start that it serves beyond this machine with no password:\n{printed}"
        );

        let status = client.auth_status().await;
        assert_eq!(status["password_set"], json!(false), "{status}");
        assert_eq!(status["no_auth_warning"], json!(true), "{status}");

        let before = dux.config_text().await;
        let dismissed = client
            .post_empty("/api/v1/auth/dismiss-no-auth-warning")
            .await;
        assert_eq!(dismissed.status, 204, "{}", dismissed.describe());
        let after = dux.config_text().await;
        assert!(after.contains("disable_no_auth_warning = true"), "{after}");
        for line in before.lines().filter(|l| l.trim_start().starts_with('#')) {
            assert!(after.contains(line), "the write kept the comment {line:?}");
        }
        assert_eq!(client.auth_status().await["no_auth_warning"], json!(false));
    })
    .await;
}
