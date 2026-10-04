//! Reverse proxies in front of a dux: nginx and Caddy, each in a container of
//! its own that SHARES the dux container's network namespace. Sharing it is what
//! puts the proxy where a person's proxy sits, on the same machine as dux,
//! talking to it over loopback. The proxy listens on a port the dux container
//! published ([`crate::DuxOptions::with_published`]), because a container in
//! another's namespace cannot publish ports of its own.

use std::time::Duration;

use testcontainers::bollard::models::HostConfig;
use testcontainers::core::WaitFor;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::dux::Dux;
use crate::util::{eventually, suffix};

/// The nginx image the proxy journeys pull. Pinned to a major line so a new
/// release cannot change what a forwarding header looks like under a journey.
pub const NGINX_IMAGE: (&str, &str) = ("nginx", "1-alpine");

/// The Caddy image the TLS journeys pull.
pub const CADDY_IMAGE: (&str, &str) = ("caddy", "2-alpine");

/// One proxy container. Removed when dropped.
pub struct Sidecar {
    container: ContainerAsync<GenericImage>,
    name: String,
}

async fn start_in(
    dux: &Dux,
    image: (&str, &str),
    config_path: &str,
    config: &str,
    ready: WaitFor,
) -> Sidecar {
    let name = format!("dux-journeys-{}-{}", image.0, suffix());
    let namespace = format!("container:{}", dux.id());
    let container = GenericImage::new(image.0, image.1)
        .with_wait_for(ready)
        .with_container_name(&name)
        .with_label("dux-journeys", "1")
        .with_copy_to(config_path.to_string(), config.as_bytes().to_vec())
        .with_host_config_modifier(move |host: &mut HostConfig| {
            host.network_mode = Some(namespace.clone());
            host.publish_all_ports = Some(false);
            host.port_bindings = None;
        })
        .start()
        .await
        .unwrap_or_else(|err| panic!("start the {} sidecar {name}: {err}", image.0));
    Sidecar { container, name }
}

impl Sidecar {
    /// nginx with `conf` as its `/etc/nginx/conf.d/default.conf`, in `dux`'s
    /// network namespace. Waits until every port in `ports` answers.
    pub async fn nginx(dux: &Dux, conf: &str, ports: &[u16]) -> Sidecar {
        let sidecar = start_in(
            dux,
            NGINX_IMAGE,
            "/etc/nginx/conf.d/default.conf",
            conf,
            WaitFor::millis(200),
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
            CADDY_IMAGE,
            "/etc/caddy/Caddyfile",
            caddyfile,
            WaitFor::millis(200),
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

/// Wait until `port` inside the shared namespace accepts a request.
async fn wait_listening(dux: &Dux, port: u16, scheme: &str) {
    eventually(
        &format!("a sidecar listening on {scheme} port {port}"),
        Duration::from_secs(30),
        || async {
            let run = dux
                .exec(&format!(
                    "curl -ks -o /dev/null --max-time 2 {scheme}://127.0.0.1:{port}/healthz"
                ))
                .await;
            (run.code == 0).then_some(())
        },
    )
    .await;
}
