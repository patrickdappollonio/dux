//! Reverse proxies in front of a dux: nginx and Caddy, each in a container of
//! its own that SHARES the dux container's network namespace. Sharing it is what
//! puts the proxy where a person's proxy sits, on the same machine as dux,
//! talking to it over loopback. The proxy listens on a port the dux container
//! published ([`crate::DuxOptions::with_published`]), because a container in
//! another's namespace cannot publish ports of its own.

use std::time::Duration;

use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::container::{identity, labelled, share_namespace_of};
use crate::dux::Dux;
use crate::image::{Reaper, pinned_parts};
use crate::util::eventually;

/// One proxy container. Removed when dropped.
pub struct Sidecar {
    container: ContainerAsync<GenericImage>,
    _reaper: Reaper,
    name: String,
}

/// Start `image_key` (a pinned image from journey-images.env) with `config`
/// copied to `config_path`, inside `dux`'s network namespace.
async fn start_in(
    dux: &Dux,
    image_key: &str,
    kind: &str,
    config_path: &str,
    config: &str,
) -> Sidecar {
    let (image, tag) = pinned_parts(image_key);
    let (name, reaper, logs) = identity(kind);
    let request = GenericImage::new(image, tag).with_wait_for(WaitFor::millis(200));
    let container = labelled(request.into(), &name, &logs)
        .with_copy_to(config_path.to_string(), config.as_bytes().to_vec())
        .with_host_config_modifier(share_namespace_of(dux.id()))
        .start()
        .await
        .unwrap_or_else(|err| panic!("start the {kind} sidecar {name}: {err}"));
    Sidecar {
        container,
        _reaper: reaper,
        name,
    }
}

impl Sidecar {
    /// nginx with `conf` as its `/etc/nginx/conf.d/default.conf`, in `dux`'s
    /// network namespace. Waits until every port in `ports` answers.
    pub async fn nginx(dux: &Dux, conf: &str, ports: &[u16]) -> Sidecar {
        let sidecar = start_in(
            dux,
            "JOURNEY_NGINX_IMAGE",
            "nginx",
            "/etc/nginx/conf.d/default.conf",
            conf,
        )
        .await;
        for port in ports {
            wait_listening(dux, *port, "http").await;
        }
        sidecar
    }

    /// Caddy with `caddyfile`, in `dux`'s network namespace. Waits until every
    /// port in `ports` answers TLS.
    pub async fn caddy(dux: &Dux, caddyfile: &str, ports: &[u16]) -> Sidecar {
        let sidecar = start_in(
            dux,
            "JOURNEY_CADDY_IMAGE",
            "caddy",
            "/etc/caddy/Caddyfile",
            caddyfile,
        )
        .await;
        for port in ports {
            wait_listening(dux, *port, "https").await;
        }
        sidecar
    }

    /// The certificate authority Caddy's `tls internal` signs with, as PEM, for
    /// a client that should trust it.
    pub async fn caddy_root(&self) -> Vec<u8> {
        eventually(
            "Caddy's local certificate authority",
            Duration::from_secs(30),
            || async {
                let mut run = self
                    .container
                    .exec(testcontainers::core::ExecCommand::new([
                        "cat",
                        "/data/caddy/pki/authorities/local/root.crt",
                    ]))
                    .await
                    .ok()?;
                let pem = run.stdout_to_vec().await.ok()?;
                pem.starts_with(b"-----BEGIN").then_some(pem)
            },
        )
        .await
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Wait until something listens on `port` inside the shared namespace.
async fn wait_listening(dux: &Dux, port: u16, scheme: &str) {
    eventually(
        &format!("a sidecar listening on {scheme} port {port}"),
        Duration::from_secs(30),
        || async {
            dux.listening()
                .await
                .iter()
                .any(|address| address.ends_with(&format!(":{port}")))
                .then_some(())
        },
    )
    .await;
}

/// The port Caddy's stand-in for `tailscale serve` listens on (in dux's
/// namespace; published by dux).
pub const SERVE_PORT: u16 = 8443;

/// The port Caddy's ordinary HTTPS reverse proxy listens on.
pub const PROXY_PORT: u16 = 9443;

/// The stand-in `tailscale serve status --json`: an HTTPS route on this
/// machine's tailnet name, port 443, proxying to dux's port. It is what lets
/// dux confirm that a forwarded request came through `tailscale serve`.
pub fn serve_route_json() -> String {
    format!(
        r#"{{"TCP":{{"443":{{"HTTPS":true}}}},"Web":{{"{name}:443":{{"Handlers":{{"/":{{"Proxy":"http://127.0.0.1:{port}"}}}}}}}}}}"#,
        name = crate::TAILNET_NAME,
        port = crate::DUX_PORT
    )
}

/// Caddy terminating TLS beside dux, twice:
///
/// - On [`SERVE_PORT`], a stand-in for the `tailscale serve` route in
///   [`serve_route_json`]: TLS for this machine's tailnet name, and the request
///   forwarded the way Tailscale's serve proxy forwards it, with Tailscale's
///   identity headers and the `Host` and `Origin` a browser at `https://<name>/`
///   sends. The only thing the real path has that this one does not is port 443
///   on the outside, so the stand-in rewrites the ephemeral published port away.
/// - On [`PROXY_PORT`], an ordinary HTTPS reverse proxy for `127.0.0.1`: no
///   identity headers, and the `Host` the client sent.
pub fn serve_and_proxy_caddyfile() -> String {
    let name = crate::TAILNET_NAME;
    let dux = crate::DUX_PORT;
    format!(
        "{{
  admin off
  skip_install_trust
  default_sni 127.0.0.1
}}

https://{name}:{SERVE_PORT} {{
  tls internal
  reverse_proxy 127.0.0.1:{dux} {{
    header_up Host {name}
    header_up Origin https://{name}
    header_up Tailscale-User-Login \"owner@example.com\"
    header_up Tailscale-User-Name \"Owner\"
  }}
}}

https://127.0.0.1:{PROXY_PORT} {{
  tls internal
  reverse_proxy 127.0.0.1:{dux}
}}
"
    )
}

/// A client that reaches the `tailscale serve` stand-in the way a browser at
/// `https://<tailnet name>/` does: TLS for that name, trusting `root`, delivered
/// to the port dux published for it.
pub async fn serve_client(dux: &Dux, root: &[u8]) -> crate::Client {
    let port = dux.host_port(SERVE_PORT).await;
    crate::Client::with_root_at(
        &format!("https://{}:{port}", crate::TAILNET_NAME),
        root,
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
    )
}
