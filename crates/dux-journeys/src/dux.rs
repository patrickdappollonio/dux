//! One running dux in a container, and the ways a journey reaches it.
//!
//! Three kinds of client exist, and they are the three places a request to dux
//! can come from:
//!
//! - **The network**: the test process on the host, through the port Docker
//!   published. dux sees the Docker bridge's address, which is neither loopback
//!   nor a tailnet address. [`Dux::client`].
//! - **This machine**: `curl` run inside the container against loopback.
//!   [`Dux::inside`].
//! - **A tailnet peer**: the host again, through a published port the container
//!   relays onto the stand-in Tailscale address FROM a peer address of its own
//!   ([`crate::TAILNET_PEER_IP`]), so the connection arrives on dux's Tailscale
//!   listener from a tailnet address that is not dux's. [`Dux::client_on`] with
//!   a relay port.
//!
//! Every published port binds the host's loopback only (see
//! [`crate::container`]). The container is removed when the [`Dux`] drops,
//! whether the journey passed, panicked or ran out of time, and a journey that
//! fails prints everything the container said (see [`crate::journey`]).

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use testcontainers::core::{AccessMode, ExecCommand, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::client::{Client, Response, parse_raw_response};
use crate::container::{identity, labelled, loopback_publish};
use crate::image::{JourneyNetwork, Reaper, dux_binary, journey_image};
use crate::util::{LogBuffer, eventually, shell_quote, suffix};
use crate::{DUX_PORT, TAILNET_IP, TAILNET_PEER_IP};

/// dux's config directory inside the journey image (the entrypoint's
/// `DUX_HOME`).
const DUX_HOME: &str = "/data/dux";

/// Where dux listens inside its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bind {
    /// `--bind 0.0.0.0:3890`: reachable through the published port, so the
    /// host's requests arrive from the network.
    Everywhere,
    /// No `--bind`: loopback plus the Tailscale leg when the stand-in reports
    /// one, exactly as an unconfigured `dux server` listens. Nothing reaches it
    /// from the host except through a relay.
    Local,
}

/// Which serving mode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// `dux server`.
    Server,
    /// The terminal UI in tmux. Serving then needs either
    /// `[server] serve_while_tui = true` (a seed hook) or the
    /// `start-web-server` palette command (the flip, sent with [`Dux::tmux_keys`]).
    Tui,
}

/// How to start one dux. Build with [`DuxOptions::exposed`] or
/// [`DuxOptions::local`] and the `with_*` methods.
#[derive(Debug, Clone)]
pub struct DuxOptions {
    bind: Bind,
    launch: Launch,
    fake_tailscale: bool,
    restart_loop: bool,
    relays: Vec<(u16, String)>,
    published: Vec<u16>,
    env: BTreeMap<String, String>,
    hooks: Vec<(String, String)>,
    network: Option<String>,
}

impl DuxOptions {
    /// dux on every interface: the host is a client from the network.
    pub fn exposed() -> Self {
        Self::new(Bind::Everywhere)
    }

    /// dux on loopback (and the stand-in tailnet, with [`Self::with_tailnet`]).
    pub fn local() -> Self {
        Self::new(Bind::Local)
    }

    fn new(bind: Bind) -> Self {
        Self {
            bind,
            launch: Launch::Server,
            fake_tailscale: false,
            restart_loop: false,
            relays: Vec::new(),
            published: Vec::new(),
            env: BTreeMap::new(),
            hooks: Vec::new(),
            network: None,
        }
    }

    /// Put the stand-in `tailscale` CLI on PATH (answering with [`TAILNET_IP`])
    /// and relay `relay_port` onto dux's Tailscale listener from the peer's own
    /// address ([`TAILNET_PEER_IP`]), so the host can be a tailnet peer through it.
    pub fn with_tailnet(mut self, relay_port: u16) -> Self {
        self.fake_tailscale = true;
        self.relays.push((
            relay_port,
            format!("{TAILNET_IP}:{DUX_PORT},bind={TAILNET_PEER_IP}"),
        ));
        self
    }

    /// Relay `port` onto loopback with no forwarding header, the way a raw TCP
    /// forward (a `tailscale serve` TCP forward, a port forwarder) reaches dux.
    pub fn with_loopback_relay(mut self, port: u16) -> Self {
        self.relays.push((port, format!("127.0.0.1:{DUX_PORT}")));
        self
    }

    /// Publish a container port without relaying it, for a sidecar that shares
    /// this container's network namespace (nginx, Caddy) and listens on it.
    pub fn with_published(mut self, port: u16) -> Self {
        self.published.push(port);
        self
    }

    /// An environment variable for the container (and so for dux and the
    /// providers it spawns).
    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    /// A shell hook run before dux starts, after the canonical config exists.
    /// Hooks run in the order they were added.
    pub fn with_hook(mut self, script: &str) -> Self {
        let name = format!("{:02}-journey.sh", self.hooks.len() + 10);
        self.hooks.push((name, script.to_string()));
        self
    }

    /// Set the web password before dux starts, the way a person does:
    /// `dux config set server.auth.password --stdin`.
    pub fn with_password(self, password: &str) -> Self {
        let script = format!(
            "set -e\n{}\n",
            secret_command("server.auth.password", password)
        );
        self.with_hook(&script)
    }

    /// Set one config value before dux starts, through `dux config set`.
    pub fn with_config(self, path: &str, value: &str) -> Self {
        let script = format!(
            "set -e\ndux config set {} {}\n",
            shell_quote(path),
            shell_quote(value)
        );
        self.with_hook(&script)
    }

    /// Start dux again whenever it exits, so [`Dux::restart_process`] gives a
    /// new run behind the same ports and state.
    pub fn with_restart_loop(mut self) -> Self {
        self.restart_loop = true;
        self
    }

    /// Run the terminal UI in tmux instead of `dux server`.
    pub fn with_tui(mut self) -> Self {
        self.launch = Launch::Tui;
        // The terminal UI greets a first run with screens that would sit in
        // front of the palette; the screenshot driver turns them off the same way.
        self.with_hook(
            "set -e\n\
             sed -i \
               -e 's/^disable_automated_welcome_screen = false$/disable_automated_welcome_screen = true/' \
               -e 's/^disable_release_notes = false$/disable_release_notes = true/' \
               -e 's/^github_integration = true$/github_integration = false/' \
               \"$DUX_HOME/config.toml\"\n",
        )
    }

    /// Join a journey's own Docker network, so a browser container on it can
    /// reach dux by its address there.
    pub fn with_network(mut self, network: &JourneyNetwork) -> Self {
        self.network = Some(network.name().to_string());
        self
    }
}

/// The `dux config set <path> --stdin` command line, fed `secret` through a
/// pipe. Base64 on the way in so no quoting can change a byte of it, and never
/// on the command line of dux itself.
fn secret_command(path: &str, secret: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(secret.as_bytes());
    format!(
        "printf %s {} | base64 -d | dux config set {} --stdin",
        shell_quote(&encoded),
        shell_quote(path)
    )
}

/// What a command run inside the container did.
#[derive(Debug, Clone)]
pub struct Exec {
    pub code: i64,
    pub stdout: String,
    pub stderr: String,
}

impl Exec {
    /// Both streams, for an assertion message or a substring check.
    pub fn output(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// One running dux. Dropping it removes the container.
pub struct Dux {
    container: ContainerAsync<GenericImage>,
    // After the container, so it drops second: it removes a container a
    // cancelled start never handed over, and is a no-op otherwise.
    _reaper: Reaper,
    // Last, so it drops after the container that was on it: the network this
    // dux made for itself when the journey named none.
    _own_network: Option<JourneyNetwork>,
    logs: LogBuffer,
    bind: Bind,
    name: String,
}

impl Dux {
    /// Start a dux and wait until it answers.
    pub async fn start(options: DuxOptions) -> Dux {
        let (image_name, tag) = journey_image().await;
        // A network of its own unless the journey shares one, so no journey
        // container ever sits on Docker's default bridge beside unrelated
        // containers. Made before the reaper, so a cancelled start drops the
        // reaper (removing the container) first and the network after it.
        let own_network = match options.network {
            Some(_) => None,
            None => Some(JourneyNetwork::create()),
        };
        let network = options
            .network
            .clone()
            .or_else(|| own_network.as_ref().map(|n| n.name().to_string()))
            .expect("a network");
        let (name, reaper, logs) = identity("dux");

        let mut ports = vec![DUX_PORT];
        ports.extend(options.relays.iter().map(|(port, _)| *port));
        ports.extend(options.published.iter().copied());
        let mut image = GenericImage::new(image_name, tag)
            .with_wait_for(WaitFor::message_on_stdout("entrypoint: "));
        for port in &ports {
            image = image.with_exposed_port(port.tcp());
        }

        let binary = dux_binary();
        let mut request = labelled(image.into(), &name, &logs)
            .with_host_config_modifier(loopback_publish(ports))
            .with_mount(
                Mount::bind_mount(binary.to_string_lossy(), "/usr/local/bin/dux")
                    .with_access_mode(AccessMode::ReadOnly),
            )
            .with_env_var("DUX_PORT", DUX_PORT.to_string())
            .with_env_var("DUX_TAIL_LOG", "1")
            .with_startup_timeout(Duration::from_secs(120));

        request = match options.bind {
            Bind::Everywhere => request,
            Bind::Local => request.with_env_var("DUX_BIND", "local"),
        };
        if options.launch == Launch::Tui {
            request = request.with_env_var("DUX_LAUNCH", "tui");
        }
        if options.fake_tailscale {
            request = request
                .with_env_var("DUX_FAKE_TAILSCALE", "1")
                .with_cap_add("NET_ADMIN");
        }
        if options.restart_loop {
            request = request.with_env_var("DUX_RESTART_LOOP", "1");
        }
        if !options.relays.is_empty() {
            let relays: Vec<String> = options
                .relays
                .iter()
                .map(|(port, target)| format!("{port}={target}"))
                .collect();
            request = request.with_env_var("DUX_RELAYS", relays.join(" "));
        }
        for (key, value) in &options.env {
            request = request.with_env_var(key, value);
        }
        for (hook, script) in &options.hooks {
            request = request.with_copy_to(
                format!("/journey/seed.d/{hook}"),
                script.clone().into_bytes(),
            );
        }
        request = request.with_network(&network);

        let container = request
            .start()
            .await
            .unwrap_or_else(|err| panic!("start the dux container {name}: {err}"));
        let dux = Dux {
            container,
            _reaper: reaper,
            _own_network: own_network,
            logs,
            bind: options.bind,
            name,
        };
        if options.launch == Launch::Server {
            dux.wait_healthy().await;
        }
        dux
    }

    /// The container's name, for messages and for sidecars that join its
    /// network namespace.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The container's id.
    pub fn id(&self) -> &str {
        self.container.id()
    }

    /// The host port Docker published `container_port` on.
    pub async fn host_port(&self, container_port: u16) -> u16 {
        self.container
            .get_host_port_ipv4(container_port.tcp())
            .await
            .unwrap_or_else(|err| panic!("port {container_port} is not published: {err}"))
    }

    /// `http://127.0.0.1:<published>` for a container port.
    pub async fn url_on(&self, container_port: u16) -> String {
        format!("http://127.0.0.1:{}", self.host_port(container_port).await)
    }

    /// dux's own URL from the host. Only meaningful for [`Bind::Everywhere`]; a
    /// local dux is reached through a relay ([`Self::client_on`]).
    pub async fn url(&self) -> String {
        assert_eq!(
            self.bind,
            Bind::Everywhere,
            "a dux bound to loopback has no URL from the host; use a relay port"
        );
        self.url_on(DUX_PORT).await
    }

    /// A client on the network: a fresh cookie jar talking to dux's published
    /// port.
    pub async fn client(&self) -> Client {
        Client::new(&self.url().await)
    }

    /// A fresh client through a published relay or sidecar port.
    pub async fn client_on(&self, container_port: u16) -> Client {
        Client::new(&self.url_on(container_port).await)
    }

    /// A client on THIS machine: curl inside the container against loopback,
    /// with a cookie jar of its own.
    pub fn inside(&self) -> Inside<'_> {
        Inside {
            dux: self,
            jar: format!("/tmp/jar-{}", suffix()),
            base: format!("http://127.0.0.1:{DUX_PORT}"),
            headers: Vec::new(),
        }
    }

    /// Run a shell script inside the container and collect what it did.
    pub async fn exec(&self, script: &str) -> Exec {
        let mut result = self
            .container
            .exec(ExecCommand::new([
                "sh",
                "-c",
                // The entrypoint exports DUX_HOME for dux and its seed hooks, but
                // an exec is a fresh shell: without it `dux config set` would
                // find a different config than the running dux reads.
                &format!("export DUX_HOME={DUX_HOME}; {script}"),
            ]))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "exec in {} failed; if the container stopped, a seed hook or dux itself \
                     failed and its output is printed below: {err}",
                    self.name
                )
            });
        let stdout = result.stdout_to_vec().await.unwrap_or_default();
        let stderr = result.stderr_to_vec().await.unwrap_or_default();
        let code = eventually(
            "the exec to report its exit code",
            Duration::from_secs(10),
            || {
                let result = &result;
                async move { result.exit_code().await.ok().flatten() }
            },
        )
        .await;
        Exec {
            code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        }
    }

    /// [`Self::exec`], panicking with the output unless it exited 0.
    pub async fn exec_ok(&self, script: &str) -> String {
        let run = self.exec(script).await;
        assert_eq!(
            run.code,
            0,
            "`{script}` failed in {}:\n{}",
            self.name,
            run.output()
        );
        run.stdout
    }

    /// `dux config set <path> <value>`, as a person types it. Returns what it
    /// did rather than asserting, so a journey can check a refusal.
    pub async fn config_set(&self, path: &str, value: &str) -> Exec {
        self.exec(&format!(
            "dux config set {} {}",
            shell_quote(path),
            shell_quote(value)
        ))
        .await
    }

    /// `dux config set <path> --stdin` with `secret` piped in.
    pub async fn config_set_secret(&self, path: &str, secret: &str) -> Exec {
        self.exec(&secret_command(path, secret)).await
    }

    /// `dux config get <path>`.
    pub async fn config_get(&self, path: &str) -> Exec {
        self.exec(&format!("dux config get {}", shell_quote(path)))
            .await
    }

    /// The config file as it is on disk right now.
    pub async fn config_text(&self) -> String {
        self.exec_ok("cat \"$DUX_HOME/config.toml\" 2>/dev/null || cat /data/dux/config.toml")
            .await
    }

    /// Ask the running dux to re-read its config, as `kill -USR1` does after a
    /// hand edit.
    pub async fn signal_reload(&self) {
        self.exec_ok("pkill -USR1 -x dux").await;
    }

    /// Everything the container has printed so far (dux's output and dux.log).
    pub fn logs(&self) -> String {
        self.logs.text()
    }

    /// Wait until the container has printed `needle`.
    pub async fn wait_for_log(&self, needle: &str, within: Duration) {
        eventually(&format!("the log line {needle:?}"), within, || async {
            self.logs().contains(needle).then_some(())
        })
        .await;
    }

    /// Whether dux answers `/healthz` from this machine right now.
    pub async fn answers_healthz(&self) -> bool {
        self.exec(&format!(
            "curl -fsS --max-time 2 http://127.0.0.1:{DUX_PORT}/healthz"
        ))
        .await
        .code
            == 0
    }

    /// Wait until dux answers `/healthz` from this machine.
    pub async fn wait_healthy(&self) {
        eventually(
            &format!("dux in {} to answer /healthz", self.name),
            Duration::from_secs(60),
            || async {
                // A seed hook that failed (a `dux config set` refusing, say)
                // ends the container; say that rather than timing out on it.
                if !self.container.is_running().await.unwrap_or(false) {
                    panic!(
                        "the container {} stopped before dux answered; a seed hook or dux \
                         itself failed (its output is printed below)",
                        self.name
                    );
                }
                self.answers_healthz().await.then_some(())
            },
        )
        .await;
    }

    /// The process id of the running `dux`, if one is running.
    pub async fn dux_pid(&self) -> Option<u32> {
        let run = self.exec("pgrep -x dux | head -1").await;
        run.stdout.trim().parse().ok()
    }

    /// Stop the dux process inside the container and wait for the restart loop
    /// to bring up a new run that answers. Needs [`DuxOptions::with_restart_loop`].
    pub async fn restart_process(&self) {
        let before = self.dux_pid().await.expect("dux is running");
        self.exec_ok(&format!("kill -TERM {before}")).await;
        eventually("a new dux process", Duration::from_secs(30), || async {
            match self.dux_pid().await {
                Some(pid) if pid != before => Some(()),
                _ => None,
            }
        })
        .await;
        self.wait_healthy().await;
    }

    /// Stop the dux process, keep it down for `down`, then let the restart loop
    /// bring up a new run and wait until it answers. Needs
    /// [`DuxOptions::with_restart_loop`]; the loop holds while `/tmp/dux-hold`
    /// exists.
    pub async fn stop_process_for(&self, down: Duration) {
        let before = self.dux_pid().await.expect("dux is running");
        self.exec_ok(&format!("touch /tmp/dux-hold && kill -TERM {before}"))
            .await;
        eventually(
            "the dux process to exit",
            Duration::from_secs(30),
            || async { self.dux_pid().await.is_none().then_some(()) },
        )
        .await;
        tokio::time::sleep(down).await;
        self.exec_ok("rm -f /tmp/dux-hold").await;
        eventually("a new dux process", Duration::from_secs(30), || async {
            self.dux_pid().await.map(|_| ())
        })
        .await;
        self.wait_healthy().await;
    }

    /// Rewrite what the stand-in `tailscale serve status --json` answers. dux
    /// reads it at its next look (every few seconds).
    pub async fn set_fake_serve(&self, json: &str) {
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        self.exec_ok(&format!(
            "printf %s {} | base64 -d > /data/tailscale/serve.json.tmp && \
             mv /data/tailscale/serve.json.tmp /data/tailscale/serve.json",
            shell_quote(&encoded)
        ))
        .await;
    }

    /// The address dux sees for a client on the host: the Docker bridge
    /// gateway, which is this container's default route.
    pub async fn host_client_address(&self) -> String {
        self.exec_ok("ip route show default | awk '{print $3; exit}'")
            .await
            .trim()
            .to_string()
    }

    /// The connections dux has accepted on its port right now, as the kernel
    /// sees them: `(dux's local address, the peer's address)`, ports dropped.
    /// This is the peer address dux itself is handed for each connection, so a
    /// journey can prove which class of client it really was.
    pub async fn observed_peers(&self) -> Vec<(String, String)> {
        let listed = self
            .exec_ok(&format!(
                "ss -Htn state established '( sport = :{DUX_PORT} )' | awk '{{print $3, $4}}'"
            ))
            .await;
        listed
            .lines()
            .filter_map(|line| {
                let (local, peer) = line.split_once(' ')?;
                Some((strip_port(local), strip_port(peer)))
            })
            .collect()
    }

    /// The addresses dux is listening on, ports included (`127.0.0.1:3890`).
    pub async fn listening(&self) -> Vec<String> {
        self.exec_ok("ss -Htln | awk '{print $4}'")
            .await
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The host's own `docker inspect` view of this container's published
    /// ports, for the test that nothing is published beyond loopback.
    pub async fn published_bindings(&self) -> Vec<(String, String, String)> {
        crate::container::published_bindings(self.id()).await
    }

    /// This container's address on its Docker network, for a browser or proxy
    /// container on the same network. An IP literal, because dux's Host guard
    /// admits one for a wildcard bind and refuses a name nobody configured.
    pub async fn address(&self) -> String {
        self.exec_ok("ip -4 -o addr show dev eth0 | awk '{print $4}' | cut -d/ -f1 | head -1")
            .await
            .trim()
            .to_string()
    }

    /// Send keys to the terminal UI's tmux session (tmux key names: `C-p`,
    /// `Enter`, ...).
    pub async fn tmux_keys(&self, keys: &[&str]) {
        let keys: Vec<String> = keys.iter().map(|k| shell_quote(k)).collect();
        self.exec_ok(&format!(
            "tmux -L journey send-keys -t journey {}",
            keys.join(" ")
        ))
        .await;
    }

    /// Type literal text into the terminal UI.
    pub async fn tmux_type(&self, text: &str) {
        self.exec_ok(&format!(
            "tmux -L journey send-keys -t journey -l {}",
            shell_quote(text)
        ))
        .await;
    }

    /// The terminal UI's screen as text.
    pub async fn tmux_screen(&self) -> String {
        self.exec("tmux -L journey capture-pane -p -t journey")
            .await
            .stdout
    }

    /// Wait until the terminal UI shows `needle`.
    pub async fn wait_for_screen(&self, needle: &str, within: Duration) {
        eventually(
            &format!("the screen to show {needle:?}"),
            within,
            || async { self.tmux_screen().await.contains(needle).then_some(()) },
        )
        .await;
    }
}

/// `1.2.3.4:5` or `[::1]:5` without its port, and an IPv4-mapped IPv6 address
/// as the IPv4 address it is.
fn strip_port(address: &str) -> String {
    let host = address
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(address)
        .trim_start_matches('[')
        .trim_end_matches(']');
    host.trim_start_matches("::ffff:").to_string()
}

/// A client on this machine: `curl` inside the dux container, against loopback,
/// with its own cookie jar file.
pub struct Inside<'a> {
    dux: &'a Dux,
    jar: String,
    base: String,
    headers: Vec<(String, String)>,
}

impl Inside<'_> {
    /// Send this header on every request from now on.
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Talk to another loopback port inside the container (a sidecar's).
    pub fn on_port(mut self, port: u16) -> Self {
        self.base = format!("http://127.0.0.1:{port}");
        self
    }

    pub async fn get(&self, path: &str) -> Response {
        self.request("GET", path, None).await
    }

    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> Response {
        self.request("POST", path, Some(body)).await
    }

    pub async fn post_empty(&self, path: &str) -> Response {
        self.request("POST", path, None).await
    }

    /// `POST /api/v1/auth/login` with `password`.
    pub async fn login(&self, password: &str) -> Response {
        self.post_json(
            "/api/v1/auth/login",
            &serde_json::json!({ "password": password }),
        )
        .await
    }

    /// `GET /api/v1/auth/status` as JSON.
    pub async fn auth_status(&self) -> serde_json::Value {
        let response = self.get("/api/v1/auth/status").await;
        assert_eq!(
            response.status,
            200,
            "auth status is public and always answers: {}",
            response.describe()
        );
        response.json()
    }

    /// One request, made the way a browser on this machine would: the jar,
    /// and an `Origin` naming dux itself on anything but a GET.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Response {
        let mut command = format!(
            "curl -sS -i --http1.1 --max-time 30 -c {jar} -b {jar} -X {method}",
            jar = shell_quote(&self.jar),
        );
        if method != "GET" {
            command.push_str(&format!(
                " -H {}",
                shell_quote(&format!("Origin: {}", self.base))
            ));
        }
        for (name, value) in &self.headers {
            command.push_str(&format!(" -H {}", shell_quote(&format!("{name}: {value}"))));
        }
        let script = match body {
            Some(body) => {
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(body.to_string().as_bytes());
                format!(
                    "printf %s {} | base64 -d | {command} -H 'Content-Type: application/json' \
                     --data-binary @- {}",
                    shell_quote(&encoded),
                    shell_quote(&format!("{}{path}", self.base))
                )
            }
            None => format!("{command} {}", shell_quote(&format!("{}{path}", self.base))),
        };
        let run = self.dux.exec(&script).await;
        assert_eq!(
            run.code,
            0,
            "curl {method} {path} from inside failed: {}",
            run.output()
        );
        parse_raw_response(&run.stdout)
    }
}
