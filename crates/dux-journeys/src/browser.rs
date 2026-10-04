//! A real browser for the journeys that are about what a person SEES: headless
//! Chromium driven over WebDriver by `chromedriver`, both from the journey image
//! (Arch's chromium package ships the driver), in a container of its own.
//!
//! Where the browser sits decides who dux thinks it is:
//!
//! - [`Browser::on_network`] puts it on a journey network beside dux, a separate
//!   machine as far as dux can tell: a client from the network.
//! - [`Browser::beside`] puts it in dux's own network namespace, so it reaches
//!   dux on loopback: this machine. dux must publish [`WEBDRIVER_PORT`] for it.
//!
//! chromedriver takes commands from exactly one address: the Docker gateway its
//! container sees, which is where the host's connection through the published
//! port arrives from. That port is published on the host's loopback only (see
//! [`crate::container`]), so nothing beyond this machine can reach the driver.

use std::time::Duration;

use fantoccini::{Client as WebDriver, ClientBuilder, Locator};
use hyper_util::client::legacy::connect::HttpConnector;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::container::{identity, labelled, loopback_publish, share_namespace_of};
use crate::dux::Dux;
use crate::image::{JourneyNetwork, Reaper, journey_image};
use crate::util::eventually;

/// The port chromedriver listens on inside its container.
pub const WEBDRIVER_PORT: u16 = 4444;

/// One browser. The container is removed when dropped (call [`Browser::quit`]
/// first for a clean session end).
pub struct Browser {
    pub driver: WebDriver,
    container: ContainerAsync<GenericImage>,
    _reaper: Reaper,
}

/// chromedriver, allowing commands only from the container's default gateway.
fn chromedriver_command() -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        format!(
            "gateway=$(ip route show default | awk '{{print $3; exit}}') && \
             exec chromedriver --port={WEBDRIVER_PORT} --allowed-ips=\"$gateway\""
        ),
    ]
}

impl Browser {
    /// A browser on `network`, which dux must also be on.
    pub async fn on_network(network: &JourneyNetwork) -> Browser {
        let (image, tag) = journey_image().await;
        let (name, reaper, logs) = identity("browser");
        let request = GenericImage::new(image, tag)
            .with_entrypoint("/usr/bin/env")
            .with_wait_for(WaitFor::message_on_stdout(
                "ChromeDriver was started successfully",
            ))
            .with_exposed_port(WEBDRIVER_PORT.tcp());
        let container = labelled(request.into(), &name, &logs)
            .with_host_config_modifier(loopback_publish(vec![WEBDRIVER_PORT]))
            .with_network(network.name())
            .with_shm_size(512 * 1024 * 1024)
            .with_cmd(chromedriver_command())
            .start()
            .await
            .unwrap_or_else(|err| panic!("start the browser container {name}: {err}"));
        let port = container
            .get_host_port_ipv4(WEBDRIVER_PORT.tcp())
            .await
            .expect("the WebDriver port is published");
        let driver = connect(port).await;
        Browser {
            driver,
            container,
            _reaper: reaper,
        }
    }

    /// A browser in `dux`'s own network namespace (this machine). `dux` must
    /// have been started with `with_published(WEBDRIVER_PORT)`.
    pub async fn beside(dux: &Dux) -> Browser {
        let (image, tag) = journey_image().await;
        let (name, reaper, logs) = identity("browser");
        let request = GenericImage::new(image, tag)
            .with_entrypoint("/usr/bin/env")
            .with_wait_for(WaitFor::message_on_stdout(
                "ChromeDriver was started successfully",
            ));
        let container = labelled(request.into(), &name, &logs)
            .with_host_config_modifier(share_namespace_of(dux.id()))
            .with_shm_size(512 * 1024 * 1024)
            .with_cmd(chromedriver_command())
            .start()
            .await
            .unwrap_or_else(|err| panic!("start the browser container {name}: {err}"));
        let port = dux.host_port(WEBDRIVER_PORT).await;
        let driver = connect(port).await;
        Browser {
            driver,
            container,
            _reaper: reaper,
        }
    }

    /// The browser container's id, for inspecting what it publishes.
    pub fn container_id(&self) -> &str {
        self.container.id()
    }

    /// Go to `url` and wait for the document to finish loading.
    pub async fn goto(&self, url: &str) {
        self.driver
            .goto(url)
            .await
            .unwrap_or_else(|err| panic!("navigate to {url}: {err}"));
    }

    /// Wait until an element matching `css` exists, or panic naming `what`.
    pub async fn wait_for(&self, css: &str, what: &str) -> fantoccini::elements::Element {
        self.driver
            .wait()
            .at_most(Duration::from_secs(20))
            .for_element(Locator::Css(css))
            .await
            .unwrap_or_else(|err| panic!("{what} ({css}) never appeared: {err}"))
    }

    /// Whether an element matching `css` exists and is displayed right now.
    pub async fn is_shown(&self, css: &str) -> bool {
        match self.driver.find(Locator::Css(css)).await {
            Ok(element) => element.is_displayed().await.unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Wait until no displayed element matches `css`.
    pub async fn wait_gone(&self, css: &str, what: &str) {
        eventually(what, Duration::from_secs(20), || async {
            (!self.is_shown(css).await).then_some(())
        })
        .await;
    }

    /// The page's current URL.
    pub async fn url(&self) -> String {
        self.driver
            .current_url()
            .await
            .expect("the current URL")
            .to_string()
    }

    /// The visible text of the whole page.
    pub async fn page_text(&self) -> String {
        match self.driver.find(Locator::Css("body")).await {
            Ok(body) => body.text().await.unwrap_or_default(),
            Err(_) => String::new(),
        }
    }

    /// End the WebDriver session.
    pub async fn quit(self) {
        let _ = self.driver.clone().close().await;
    }
}

async fn connect(port: u16) -> WebDriver {
    let mut capabilities = serde_json::Map::new();
    capabilities.insert(
        "goog:chromeOptions".to_string(),
        serde_json::json!({
            "args": [
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--disable-gpu",
                "--window-size=1440,900",
            ]
        }),
    );
    let url = format!("http://127.0.0.1:{port}");
    eventually("a WebDriver session", Duration::from_secs(30), || {
        let capabilities = capabilities.clone();
        let url = url.clone();
        async move {
            ClientBuilder::new(HttpConnector::new())
                .capabilities(capabilities)
                .connect(&url)
                .await
                .ok()
        }
    })
    .await
}
