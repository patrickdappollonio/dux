//! A real browser for the journeys that are about what a person SEES: headless
//! Chromium driven over WebDriver by `chromedriver`, both from the journey image
//! (Arch's chromium package ships the driver), in a container of its own.
//!
//! Where the browser sits decides who dux thinks it is:
//!
//! - [`Browser::on_network`] puts it on a Docker network beside dux, a separate
//!   machine as far as dux can tell: a client from the network.
//! - [`Browser::beside`] puts it in dux's own network namespace, so it reaches
//!   dux on loopback: this machine. dux must publish [`WEBDRIVER_PORT`] for it.

use std::time::Duration;

use fantoccini::{Client as WebDriver, ClientBuilder, Locator};
use hyper_util::client::legacy::connect::HttpConnector;
use testcontainers::bollard::models::HostConfig;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

use crate::dux::Dux;
use crate::image::journey_image;
use crate::util::{eventually, suffix};

/// The port chromedriver listens on inside its container.
pub const WEBDRIVER_PORT: u16 = 4444;

/// One browser. The session ends and the container is removed when dropped
/// (call [`Browser::quit`] first for a clean session end).
pub struct Browser {
    pub driver: WebDriver,
    _container: Option<ContainerAsync<GenericImage>>,
}

fn chromedriver_command() -> Vec<String> {
    vec![
        "chromedriver".to_string(),
        format!("--port={WEBDRIVER_PORT}"),
        // The test process is outside the container, so the driver has to take
        // a remote connection; it is reachable only through a loopback-published
        // port on the host.
        "--allowed-ips=".to_string(),
        "--allowed-origins=*".to_string(),
    ]
}

impl Browser {
    /// A browser on `network`, which `dux` must also be on.
    pub async fn on_network(network: &str) -> Browser {
        let (image, tag) = journey_image().await;
        let container = GenericImage::new(image, tag)
            .with_entrypoint("/usr/bin/env")
            .with_wait_for(WaitFor::message_on_stdout(
                "ChromeDriver was started successfully",
            ))
            .with_exposed_port(WEBDRIVER_PORT.tcp())
            .with_container_name(format!("dux-journeys-browser-{}", suffix()))
            .with_label("dux-journeys", "1")
            .with_network(network)
            .with_shm_size(512 * 1024 * 1024)
            .with_cmd(chromedriver_command())
            .start()
            .await
            .unwrap_or_else(|err| panic!("start the browser container: {err}"));
        let port = container
            .get_host_port_ipv4(WEBDRIVER_PORT.tcp())
            .await
            .expect("the WebDriver port is published");
        let driver = connect(port).await;
        Browser {
            driver,
            _container: Some(container),
        }
    }

    /// A browser in `dux`'s own network namespace (this machine). `dux` must
    /// have been started with `with_published(WEBDRIVER_PORT)`.
    pub async fn beside(dux: &Dux) -> Browser {
        let (image, tag) = journey_image().await;
        let namespace = format!("container:{}", dux.id());
        let _container = GenericImage::new(image, tag)
            .with_entrypoint("/usr/bin/env")
            .with_wait_for(WaitFor::message_on_stdout(
                "ChromeDriver was started successfully",
            ))
            .with_container_name(format!("dux-journeys-browser-{}", suffix()))
            .with_label("dux-journeys", "1")
            .with_shm_size(512 * 1024 * 1024)
            .with_cmd(chromedriver_command())
            .with_host_config_modifier(move |host: &mut HostConfig| {
                host.network_mode = Some(namespace.clone());
                host.publish_all_ports = Some(false);
                host.port_bindings = None;
            })
            .start()
            .await
            .unwrap_or_else(|err| panic!("start the browser container: {err}"));
        let port = dux.host_port(WEBDRIVER_PORT).await;
        let driver = connect(port).await;
        Browser {
            driver,
            _container: Some(_container),
        }
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
        let _ = self.driver.close().await;
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
