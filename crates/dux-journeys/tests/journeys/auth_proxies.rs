//! dux behind a reverse proxy: journeys 7 (nginx) and 8 (HTTPS through Caddy)
//! of the login plan.

use std::time::Duration;

use dux_journeys::sidecars::Sidecar;
use dux_journeys::{Client, DUX_PORT, Dux, DuxOptions, STRONG_PASSWORD, eventually, journey};
use serde_json::json;

use crate::auth_login::{assert_auth_required, session_cookie};

/// An nginx in front of dux on the same machine: port 8080 forwards the way a
/// careful operator configures it (`X-Forwarded-For` appended), port 8081 the
/// way a careless one does (no forwarding header at all).
fn nginx_conf() -> String {
    format!(
        "server {{
  listen 8080;
  location / {{
    proxy_pass http://127.0.0.1:{DUX_PORT};
    proxy_set_header Host $http_host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
  }}
}}
server {{
  listen 8081;
  location / {{
    proxy_pass http://127.0.0.1:{DUX_PORT};
    proxy_set_header Host $http_host;
  }}
}}
"
    )
}

/// How many times dux printed the proxy warning (the one that suggests
/// `require = "everywhere"`). dux's own output and dux.log are both mirrored
/// into the container's output, so a warning written to both appears twice
/// there; the count is the larger of the two sources, not their sum.
fn proxy_warnings(dux: &Dux) -> usize {
    let is_warning = |l: &str| {
        let l = l.to_ascii_lowercase();
        l.contains("everywhere") && (l.contains("proxy") || l.contains("forward"))
    };
    let logs = dux.logs();
    let (from_log, printed): (Vec<&str>, Vec<&str>) =
        logs.lines().partition(|l| l.starts_with("dux.log: "));
    let in_log = from_log.into_iter().filter(|l| is_warning(l)).count();
    let on_screen = printed.into_iter().filter(|l| is_warning(l)).count();
    in_log.max(on_screen)
}

/// Situation: dux listening on loopback with a password and the default
/// `require = "network"`, and nginx on the same machine in front of it: one
/// server block that appends `X-Forwarded-For`, and one misconfigured block that
/// adds no forwarding header.
///
/// Task: a person reaches dux through the proxy from another machine; dux must
/// treat them as the network, not as this machine, must not be fooled by headers
/// the client forges, must warn the owner once about the proxy, and must give the
/// owner a setting that catches a proxy it cannot see through.
///
/// Action: list projects through the careful proxy with no session; again
/// claiming `X-Forwarded-For: 127.0.0.1`; again sending a Tailscale identity
/// header; read dux's output; list through the misconfigured proxy; set
/// `require = "everywhere"` and list through it again; sign in through it.
///
/// Result: every request through the careful proxy is refused with 401
/// `auth_required` (the rightmost forwarded address is the client's real one,
/// and a client-sent Tailscale header proves nothing); dux printed exactly one
/// warning about forwarded requests suggesting `require = "everywhere"`, however
/// many forwarded requests followed; the misconfigured proxy is
/// indistinguishable from this machine under `network` (the request answers),
/// which is what that warning is about; under `everywhere` it is refused too, and
/// signing in through it works.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_07_behind_nginx_the_password_still_applies() {
    journey("07-nginx", Duration::from_secs(240), async {
        let dux = Dux::start(
            DuxOptions::local()
                .with_published(8080)
                .with_published(8081)
                .with_password(STRONG_PASSWORD),
        )
        .await;
        let _nginx = Sidecar::nginx(&dux, &nginx_conf(), &[8080, 8081]).await;
        let careful = dux.client_on(8080).await;
        let careless = dux.client_on(8081).await;

        assert_eq!(
            proxy_warnings(&dux),
            0,
            "no warning before any forwarded request"
        );
        assert_auth_required(&careful.get("/api/v1/projects").await, "through nginx");
        let forged = careful.fresh().with_header("X-Forwarded-For", "127.0.0.1");
        assert_auth_required(
            &forged.get("/api/v1/projects").await,
            "through nginx claiming to be loopback",
        );
        let posing = careful
            .fresh()
            .with_header("Tailscale-User-Login", "owner@example.com")
            .with_header("Tailscale-User-Name", "Owner");
        assert_auth_required(
            &posing.get("/api/v1/projects").await,
            "through nginx with a client-sent Tailscale identity",
        );
        eventually("the proxy warning", Duration::from_secs(10), || async {
            (proxy_warnings(&dux) >= 1).then_some(())
        })
        .await;
        for _ in 0..3 {
            careful.get("/api/v1/projects").await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(proxy_warnings(&dux), 1, "the proxy warning is printed once");

        let looks_local = careless.get("/api/v1/projects").await;
        assert_eq!(
            looks_local.status,
            200,
            "a proxy that adds no header looks like this machine under require = network: {}",
            looks_local.describe()
        );

        let run = dux.config_set("server.auth.require", "everywhere").await;
        assert_eq!(run.code, 0, "{}", run.output());
        eventually(
            "require = everywhere to apply",
            Duration::from_secs(20),
            || async { (careless.get("/api/v1/projects").await.status == 401).then_some(()) },
        )
        .await;
        assert_auth_required(
            &careless.get("/api/v1/projects").await,
            "through the misconfigured proxy under everywhere",
        );
        careless.login_ok(STRONG_PASSWORD).await;
        assert_eq!(careless.get("/api/v1/projects").await.status, 200);
    })
    .await;
}

/// The stand-in `tailscale serve status --json`: an HTTPS route on this
/// machine's tailnet name proxying to dux's port, which is what lets dux confirm
/// that a forwarded request with Tailscale's identity headers came through it.
fn serve_route() -> String {
    format!(
        r#"{{"TCP":{{"443":{{"HTTPS":true}}}},"Web":{{"journey-box.example-tailnet.ts.net:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:{DUX_PORT}"}}}}}}}}}}"#
    )
}

/// Caddy terminating TLS beside dux: port 8443 stands in for `tailscale serve`
/// (it forwards with Tailscale's identity headers, as the real serve proxy
/// does), port 9443 is an ordinary HTTPS reverse proxy with no identity.
fn caddyfile() -> String {
    format!(
        "{{
  admin off
  skip_install_trust
  default_sni 127.0.0.1
}}

https://127.0.0.1:8443 {{
  tls internal
  reverse_proxy 127.0.0.1:{DUX_PORT} {{
    header_up Tailscale-User-Login \"owner@example.com\"
    header_up Tailscale-User-Name \"Owner\"
  }}
}}

https://127.0.0.1:9443 {{
  tls internal
  reverse_proxy 127.0.0.1:{DUX_PORT}
}}
"
    )
}

/// Situation: dux on loopback with a password and `require = "everywhere"`, a
/// stand-in Tailscale whose serve configuration routes HTTPS to dux's port,
/// and Caddy terminating TLS in front of dux twice: once standing in for that
/// `tailscale serve` route, once as an ordinary HTTPS proxy.
///
/// Task: a session cookie must be `Secure` when dux knows the browser reached it
/// over HTTPS, must not be when it did not, and the owner must be able to force
/// it for a proxy dux cannot vouch for.
///
/// Action: sign in over HTTPS through the `tailscale serve` stand-in; sign in
/// over plain HTTP from this machine; sign in over HTTPS through the ordinary
/// proxy; set `server.auth.cookie_secure = "always"` and sign in through the
/// ordinary proxy again.
///
/// Result: through the confirmed `tailscale serve` route the cookie is `Secure`
/// and the status reports an encrypted transport; over plain HTTP it is not
/// `Secure`; through the ordinary proxy under `cookie_secure = "auto"` it is not
/// `Secure` either, because dux never trusts an arbitrary `X-Forwarded-Proto`;
/// with `always` it is.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_08_the_cookie_is_secure_exactly_when_dux_knows_it_is_https() {
    journey("08-https", Duration::from_secs(240), async {
        let serve = serve_route();
        let dux = Dux::start(
            DuxOptions::local()
                .with_tailnet(4100)
                .with_published(8443)
                .with_published(9443)
                .with_hook(&format!(
                    "set -e\nmkdir -p /data/tailscale\nprintf '%s' '{serve}' > /data/tailscale/serve.json\n"
                ))
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.require", "everywhere"),
        )
        .await;
        let caddy = Sidecar::caddy(&dux, &caddyfile(), &[8443, 9443]).await;
        let root = caddy.caddy_root().await;
        let serve_port = dux.host_port(8443).await;
        let proxy_port = dux.host_port(9443).await;

        let via_serve = Client::with_root(&format!("https://127.0.0.1:{serve_port}"), &root);
        let status = via_serve.auth_status().await;
        assert_eq!(status["transport_encrypted"], json!(true), "{status}");
        let login = via_serve.login(STRONG_PASSWORD).await;
        assert_eq!(login.status, 204, "{}", login.describe());
        let (_, attributes) = session_cookie(&login);
        assert!(
            attributes.contains(&"secure".to_string()),
            "HTTPS through a confirmed tailscale serve route gets a Secure cookie: {attributes:?}"
        );
        assert_eq!(via_serve.get("/api/v1/projects").await.status, 200);

        let plain = dux.inside().login(STRONG_PASSWORD).await;
        assert_eq!(plain.status, 204, "{}", plain.describe());
        let (_, attributes) = session_cookie(&plain);
        assert!(
            !attributes.contains(&"secure".to_string()),
            "plain HTTP gets no Secure cookie: {attributes:?}"
        );

        let via_proxy = Client::with_root(&format!("https://127.0.0.1:{proxy_port}"), &root);
        let login = via_proxy.login(STRONG_PASSWORD).await;
        assert_eq!(login.status, 204, "{}", login.describe());
        let (_, attributes) = session_cookie(&login);
        assert!(
            !attributes.contains(&"secure".to_string()),
            "an arbitrary proxy's X-Forwarded-Proto is not trusted under auto: {attributes:?}"
        );

        let run = dux.config_set("server.auth.cookie_secure", "always").await;
        assert_eq!(run.code, 0, "{}", run.output());
        eventually("cookie_secure = always to apply", Duration::from_secs(20), || async {
            let login = via_proxy.fresh().login(STRONG_PASSWORD).await;
            (login.status == 204)
                .then(|| session_cookie(&login).1)
                .filter(|attributes| attributes.contains(&"secure".to_string()))
        })
        .await;
    })
    .await;
}
