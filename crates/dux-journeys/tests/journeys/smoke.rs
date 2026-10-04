//! The journeys that work today, with no password anywhere: they prove the
//! harness itself (the container, the three kinds of client, the sockets, the
//! sidecars and the browser) before any login journey leans on it.

use std::time::Duration;

use dux_journeys::api::{add_demo_project, create_agent, projects, session_id};
use dux_journeys::browser::{Browser, WEBDRIVER_PORT};
use dux_journeys::sidecars::Sidecar;
use dux_journeys::util::suffix;
use dux_journeys::ws::connect_ok;
use dux_journeys::{Client, DUX_PORT, Dux, DuxOptions, journey};

/// Situation: a fresh `dux server` with no password, reachable from the
/// network, with the fake provider configured and a demo repository on disk.
///
/// Task: a person wants to see dux is alive, add their repository, start an
/// agent on it and talk to that agent.
///
/// Action: check `/healthz`; list projects (none yet); add the demo repository
/// as a project on the fake provider; create an agent; open the agent's
/// terminal socket, claim it, type `hello` and Enter; then close the socket.
///
/// Result: `/healthz` says ok; the project list grows from empty to the demo
/// project; the agent appears with a tab; the terminal shows the fake agent
/// answering `you typed hello`, which proves the keystrokes reached the process
/// and its output came back; and the socket closes cleanly.
#[tokio::test(flavor = "multi_thread")]
async fn smoke_add_a_project_start_an_agent_and_say_hello() {
    journey("smoke", Duration::from_secs(240), async {
        let dux =
            Dux::start(DuxOptions::exposed().with_env("DUX_FAKE_FIXTURE", "quit-on-command")).await;
        let client = dux.client().await;

        let health = client.get("/healthz").await;
        assert_eq!(health.status, 200, "{}", health.describe());
        assert_eq!(health.body.trim(), "ok");

        assert!(
            projects(&client).await.is_empty(),
            "a fresh dux starts with no projects"
        );
        let project = add_demo_project(&client).await;
        let listed = projects(&client).await;
        assert!(
            listed
                .iter()
                .any(|p| p["id"].as_str() == Some(project.as_str())),
            "the new project is listed: {listed:?}"
        );

        let name = format!("smoke-{}", suffix());
        let session = create_agent(&client, &project, &name).await;
        let id = session_id(&session);

        let mut pty = connect_ok(&client, &format!("/ws/sessions/{id}/pty")).await;
        let hello = pty
            .next_event("connected", Duration::from_secs(20))
            .await
            .expect("the PTY socket says it is connected");
        assert!(
            hello["id"].is_string(),
            "the handshake names this connection: {hello}"
        );
        pty.claim(24, 80).await;
        assert!(
            pty.read_until("type \"quit\"", Duration::from_secs(20))
                .await,
            "the fake agent greets the terminal; got {:?}",
            pty.output_text()
        );
        pty.send_bytes(b"hello\r").await;
        assert!(
            pty.read_until("you typed hello", Duration::from_secs(20))
                .await,
            "the agent answers what was typed; got {:?}",
            pty.output_text()
        );
        pty.close().await;
    })
    .await;
}

/// Situation: a `dux server` listening the way an unconfigured one does
/// (loopback, plus the Tailscale leg), on a machine where the stand-in
/// `tailscale` reports a tailnet address, with one port relayed onto that
/// address and one onto loopback.
///
/// Task: the harness has to be able to reach dux as this machine, as a tailnet
/// peer, and through a headerless forward, before any journey can ask how dux
/// treats each of them.
///
/// Action: ask `/healthz` from inside the container on loopback, through the
/// tailnet relay, and through the loopback relay; open the events socket
/// through the tailnet relay and keep it open for a few seconds.
///
/// Result: all three answer `ok`, which shows the Tailscale listener really
/// bound on the stand-in address (the relay onto it has nowhere else to land),
/// and the events socket stays open.
#[tokio::test(flavor = "multi_thread")]
async fn smoke_this_machine_tailnet_and_relay_clients_all_reach_dux() {
    journey("smoke-reach", Duration::from_secs(180), async {
        let dux = Dux::start(
            DuxOptions::local()
                .with_tailnet(4100)
                .with_loopback_relay(4200),
        )
        .await;

        let inside = dux.inside().get("/healthz").await;
        assert_eq!((inside.status, inside.body.trim()), (200, "ok"));

        // The Tailscale leg binds a moment after the first look at the CLI.
        let tailnet = dux.client_on(4100).await;
        dux_journeys::eventually(
            "the Tailscale leg to answer",
            Duration::from_secs(30),
            || async {
                let probe = tailnet_probe(&tailnet).await;
                probe.then_some(())
            },
        )
        .await;

        let relayed = dux.client_on(4200).await.get("/healthz").await;
        assert_eq!((relayed.status, relayed.body.trim()), (200, "ok"));

        let mut events = connect_ok(&tailnet, "/ws/events").await;
        assert_eq!(
            events.hold_open(Duration::from_secs(3)).await,
            None,
            "the events socket stays open"
        );
        events.close().await;
    })
    .await;
}

async fn tailnet_probe(client: &Client) -> bool {
    // A relay with nothing listening behind it accepts and then drops the
    // connection, which reqwest reports as an error; poll on a raw TCP
    // exchange so that is a "not yet" instead of a panic.
    let url = client.base().clone();
    let addr = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap());
    let Ok(mut stream) = tokio::net::TcpStream::connect(&addr).await else {
        return false;
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut answer)).await;
    String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 200")
}

/// Situation: dux on loopback with nginx and Caddy beside it (sharing its
/// network namespace, as a proxy on the same machine does), nginx on plain HTTP
/// and Caddy terminating TLS with its own local certificate authority.
///
/// Task: the proxy journeys need both proxies to really carry requests to dux.
///
/// Action: ask `/healthz` through nginx, and through Caddy over HTTPS trusting
/// Caddy's certificate authority.
///
/// Result: both answer `ok`.
#[tokio::test(flavor = "multi_thread")]
async fn smoke_nginx_and_caddy_carry_requests_to_dux() {
    journey("smoke-proxies", Duration::from_secs(240), async {
        let dux = Dux::start(
            DuxOptions::local()
                .with_published(8080)
                .with_published(8443),
        )
        .await;
        let _nginx = Sidecar::nginx(&dux, &nginx_conf(), &[8080]).await;
        let caddy = Sidecar::caddy(&dux, &caddyfile(), &[8443]).await;

        let plain = dux.client_on(8080).await.get("/healthz").await;
        assert_eq!((plain.status, plain.body.trim()), (200, "ok"));

        let root = caddy.caddy_root().await;
        let port = dux.host_port(8443).await;
        let tls = Client::with_root(&format!("https://127.0.0.1:{port}"), &root)
            .get("/healthz")
            .await;
        assert_eq!((tls.status, tls.body.trim()), (200, "ok"));
    })
    .await;
}

fn nginx_conf() -> String {
    format!(
        "server {{\n  listen 8080;\n  location / {{\n    proxy_pass http://127.0.0.1:{DUX_PORT};\n    proxy_set_header Host $http_host;\n    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n  }}\n}}\n"
    )
}

fn caddyfile() -> String {
    format!(
        "{{\n  admin off\n  skip_install_trust\n  default_sni 127.0.0.1\n}}\n\nhttps://127.0.0.1:8443 {{\n  tls internal\n  reverse_proxy 127.0.0.1:{DUX_PORT}\n}}\n"
    )
}

/// Situation: no password anywhere; one dux exposed on a Docker network with a
/// browser container on the same network, and one dux on loopback with a
/// browser in its own network namespace.
///
/// Task: the browser journeys need a real browser that loads dux's web UI both
/// from another machine and from this one.
///
/// Action: open dux's address in each Chromium.
///
/// Result: both pages load dux's single-page app (its root element renders).
#[tokio::test(flavor = "multi_thread")]
async fn smoke_a_browser_loads_the_web_ui_from_the_network_and_this_machine() {
    journey("smoke-browser", Duration::from_secs(300), async {
        let network = format!("dux-journeys-net-{}", suffix());
        let dux = Dux::start(DuxOptions::exposed().with_network(&network)).await;
        let browser = Browser::on_network(&network).await;
        let address = dux.address().await;
        browser.goto(&format!("http://{address}:{DUX_PORT}/")).await;
        browser.wait_for("#root > *", "the web UI's root").await;
        browser.quit().await;

        let local = Dux::start(DuxOptions::local().with_published(WEBDRIVER_PORT)).await;
        let browser = Browser::beside(&local).await;
        browser.goto(&format!("http://127.0.0.1:{DUX_PORT}/")).await;
        browser
            .wait_for("#root > *", "the web UI's root on loopback")
            .await;
        browser.quit().await;
    })
    .await;
}

/// Situation: dux served from the terminal UI, the two ways a TUI serves:
/// `[server] serve_while_tui = true`, and the `start-web-server` palette
/// command (the flip).
///
/// Task: the serving-mode journeys need both TUI modes to really serve.
///
/// Action: start the TUI with `serve_while_tui` on and ask `/healthz` from this
/// machine; start a second TUI, run `start-web-server` from its palette, and ask
/// `/healthz` again.
///
/// Result: both answer `ok`.
#[tokio::test(flavor = "multi_thread")]
async fn smoke_both_terminal_ui_serving_modes_serve() {
    journey("smoke-tui-modes", Duration::from_secs(300), async {
        let background = Dux::start(
            DuxOptions::local()
                .with_tui()
                // A hand edit rather than `dux config set`: this journey has
                // to pass before that command exists.
                .with_hook(
                    "set -e\n\
                     sed -i 's/^serve_while_tui = false$/serve_while_tui = true/' \"$DUX_HOME/config.toml\"\n\
                     grep -q '^serve_while_tui = true$' \"$DUX_HOME/config.toml\"\n",
                ),
        )
        .await;
        background.wait_healthy().await;

        let flip = Dux::start(DuxOptions::local().with_tui()).await;
        flip.wait_for_screen("dux", Duration::from_secs(30)).await;
        assert!(
            !flip.answers_healthz().await,
            "the terminal UI serves nothing until it is asked to"
        );
        start_web_server(&flip).await;
        flip.wait_healthy().await;
    })
    .await;
}

/// Run `start-web-server` from the terminal UI's command palette.
pub async fn start_web_server(dux: &Dux) {
    dux.wait_for_screen("dux", Duration::from_secs(30)).await;
    dux.tmux_keys(&["C-p"]).await;
    dux.wait_for_screen("Command Palette", Duration::from_secs(15))
        .await;
    dux.tmux_type("start-web-server").await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    dux.tmux_keys(&["Enter"]).await;
}

/// Situation: `dux server` reachable from the network, run under the restart
/// loop, with one project added.
///
/// Task: the restart journeys need to stop dux inside its container, briefly
/// and for a chosen stretch, and get a new run back behind the same port with
/// the same state.
///
/// Action: restart the dux process; list projects; stop it for three seconds;
/// list projects again.
///
/// Result: each time a new dux process answers on the same published port and
/// the project is still listed; and the address dux sees for the host is an
/// IPv4 address that is not loopback.
#[tokio::test(flavor = "multi_thread")]
async fn smoke_dux_restarts_behind_the_same_port_with_the_same_state() {
    journey("smoke-restart", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed().with_restart_loop()).await;
        let client = dux.client().await;
        let project = add_demo_project(&client).await;
        let listed = |list: Vec<serde_json::Value>| {
            list.iter()
                .any(|p| p["id"].as_str() == Some(project.as_str()))
        };

        let first = dux.dux_pid().await.expect("dux is running");
        dux.restart_process().await;
        let second = dux.dux_pid().await.expect("dux is running again");
        assert_ne!(first, second, "a new dux process");
        assert!(
            listed(projects(&client).await),
            "the project survives a restart"
        );

        dux.stop_process_for(Duration::from_secs(3)).await;
        assert!(
            listed(projects(&client).await),
            "the project survives an outage"
        );

        let seen: std::net::Ipv4Addr = dux
            .host_client_address()
            .await
            .parse()
            .expect("the host's address as dux sees it");
        assert!(!seen.is_loopback(), "{seen}");
    })
    .await;
}
