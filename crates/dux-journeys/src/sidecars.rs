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
async fn start_in(dux: &Dux, image_key: &str, kind: &str, files: &[(String, Vec<u8>)]) -> Sidecar {
    let (image, tag) = pinned_parts(image_key);
    let (name, reaper, logs) = identity(kind);
    let request = GenericImage::new(image, tag).with_wait_for(WaitFor::millis(200));
    let mut request = labelled(request.into(), &name, &logs);
    for (path, bytes) in files {
        request = request.with_copy_to(path.clone(), bytes.clone());
    }
    let container = request
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
            &[(
                "/etc/nginx/conf.d/default.conf".to_string(),
                conf.as_bytes().to_vec(),
            )],
        )
        .await;
        for port in ports {
            wait_listening(dux, *port, "http").await;
        }
        sidecar
    }

    /// Caddy with `caddyfile`, in `dux`'s network namespace, serving `sites`
    /// (each a port and the name a client reaches it by). Waits until every
    /// site completes a TLS handshake verified against Caddy's own authority.
    ///
    /// A listening port is not readiness: Caddy binds its listeners and creates
    /// its authority's root BEFORE it issues each site's certificate, in the
    /// background, and a handshake in between is answered with an "internal
    /// error" alert, which a client reports as no response at all. That window
    /// is short, but a loaded machine widens it, and a journey hitting it is a
    /// red build (see `smoke_caddy_is_ready_only_once_its_sites_handshake`).
    pub async fn caddy(dux: &Dux, caddyfile: &str, sites: &[(u16, &str)]) -> Sidecar {
        Self::caddy_with(dux, caddyfile, sites, &[]).await
    }

    /// [`Self::caddy`] with `extra` files copied into the container before it
    /// starts, for a journey that has to shape Caddy's storage.
    pub async fn caddy_with(
        dux: &Dux,
        caddyfile: &str,
        sites: &[(u16, &str)],
        extra: &[(String, Vec<u8>)],
    ) -> Sidecar {
        let mut files = vec![(
            "/etc/caddy/Caddyfile".to_string(),
            caddyfile.as_bytes().to_vec(),
        )];
        files.extend_from_slice(extra);
        let sidecar = start_in(dux, "JOURNEY_CADDY_IMAGE", "caddy", &files).await;
        for (port, _) in sites {
            wait_listening(dux, *port, "https").await;
        }
        let root = sidecar.caddy_root().await;
        for (port, host) in sites {
            wait_handshake(dux, *port, host, &root).await;
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

/// Wait until `host` on `port` completes a TLS handshake that verifies against
/// `root`, reached through the port dux published, the way a journey's client
/// reaches it. Any HTTP answer at all means the handshake succeeded.
async fn wait_handshake(dux: &Dux, port: u16, host: &str, root: &[u8]) {
    let published = dux.host_port(port).await;
    let http = reqwest::Client::builder()
        .add_root_certificate(
            reqwest::Certificate::from_pem(root).expect("Caddy's root certificate parses"),
        )
        .resolve(
            host,
            std::net::SocketAddr::from(([127, 0, 0, 1], published)),
        )
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build the readiness client");
    let url = format!("https://{host}:{published}/");
    eventually(
        &format!("Caddy's certificate for {host} on port {port}"),
        Duration::from_secs(30),
        || async { http.get(&url).send().await.ok().map(drop) },
    )
    .await;
}

/// A certmagic storage lock on issuing `host`'s certificate, fresh as of now,
/// at the path Caddy's file storage keeps it. Caddy holds off issuing that
/// certificate until the lock goes stale (a couple of seconds), which widens
/// the window between "listening" and "has a certificate" from milliseconds
/// to seconds: what a loaded machine does by accident, made deterministic.
pub fn held_issuance_lock(host: &str) -> (String, Vec<u8>) {
    let now = rfc3339_now();
    (
        format!("/data/caddy/locks/issue_cert_{host}.lock"),
        format!("{{\"created\":\"{now}\",\"updated\":\"{now}\"}}").into_bytes(),
    )
}

/// The current UTC time as RFC 3339 with nanoseconds.
fn rfc3339_now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after 1970");
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:09}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_nanos()
    )
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

/// The sites [`serve_and_proxy_caddyfile`] serves: each port and the name a
/// client reaches it by.
pub fn serve_and_proxy_sites() -> [(u16, &'static str); 2] {
    [(SERVE_PORT, crate::TAILNET_NAME), (PROXY_PORT, "127.0.0.1")]
}

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
    // Tailscale's serve proxy reports the tailnet client's own address in
    // X-Forwarded-For, and dux proves serve by it, so the stand-in reports a
    // tailnet peer's address rather than the Docker address Caddy saw.
    let peer = crate::TAILNET_PEER_IP;
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
    header_up X-Forwarded-For {peer}
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
