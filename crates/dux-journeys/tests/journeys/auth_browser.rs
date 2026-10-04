//! What a person SEES around the login, in a real browser: journey 12 of the
//! login plan.
//!
//! These journeys find the login UI by `data-testid`, a contract with the web
//! UI that this file defines:
//!
//! | `data-testid`                | what it marks                                        |
//! |------------------------------|------------------------------------------------------|
//! | `login-form`                 | the login form (holds `input[type=password]` and a submit button) |
//! | `login-insecure-warning`     | the plain-HTTP eavesdropping warning on that page   |
//! | `no-auth-banner`             | the red no-password banner in the app               |
//! | `no-auth-banner-never`       | its "don't show again" control                      |
//! | `weak-password-banner`       | the banner after signing in with a below-minimum password |

use std::time::Duration;

use dux_journeys::browser::{Browser, WEBDRIVER_PORT};
use dux_journeys::image::JourneyNetwork;
use dux_journeys::{DUX_PORT, Dux, DuxOptions, STRONG_PASSWORD, eventually, journey};
use fantoccini::Locator;

const LOGIN_FORM: &str = "[data-testid=\"login-form\"]";
const INSECURE_WARNING: &str = "[data-testid=\"login-insecure-warning\"]";
const NO_AUTH_BANNER: &str = "[data-testid=\"no-auth-banner\"]";
const NO_AUTH_NEVER: &str = "[data-testid=\"no-auth-banner-never\"]";
const WEAK_BANNER: &str = "[data-testid=\"weak-password-banner\"]";

/// Type `password` into the login form and submit it.
async fn sign_in(browser: &Browser, password: &str) {
    let form = browser.wait_for(LOGIN_FORM, "the login form").await;
    let field = form
        .find(Locator::Css("input[type=\"password\"]"))
        .await
        .expect("the login form has a password field");
    field.send_keys(password).await.expect("type the password");
    form.find(Locator::Css("button[type=\"submit\"]"))
        .await
        .expect("the login form has a submit button")
        .click()
        .await
        .expect("submit the login form");
}

/// Situation: dux with a password, listening on every interface, and a browser
/// on another machine of the same network opening a deep link (an agent's
/// address, which is a hash route).
///
/// Task: the person signs in and must land exactly where the link pointed,
/// having been told that typing a password over plain HTTP can be overheard.
///
/// Action: open `http://<dux>/#/agent/<id>`; read the login page; sign in.
///
/// Result: the login page shows, with the plain-HTTP eavesdropping warning
/// visible; after signing in the login form is gone, the app is showing, and
/// the URL's hash is the one the link carried.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_12a_login_page_warns_on_plain_http_and_returns_to_the_link() {
    journey("12a-browser-login", Duration::from_secs(300), async {
        let network = JourneyNetwork::create();
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_network(&network)
                .with_password(STRONG_PASSWORD),
        )
        .await;
        let browser = Browser::on_network(&network).await;
        let address = dux.address().await;
        let hash = "#/agent/journey-deep-link";

        browser
            .goto(&format!("http://{address}:{DUX_PORT}/{hash}"))
            .await;
        browser.wait_for(LOGIN_FORM, "the login page").await;
        assert!(
            browser.is_shown(INSECURE_WARNING).await,
            "the plain-HTTP warning is visible on the login page from the network"
        );

        sign_in(&browser, STRONG_PASSWORD).await;
        browser
            .wait_gone(LOGIN_FORM, "the login form to go away")
            .await;
        eventually(
            "the deep link's hash to be restored",
            Duration::from_secs(15),
            || async { browser.url().await.ends_with(hash).then_some(()) },
        )
        .await;
        browser.quit().await;
    })
    .await;
}

/// Situation: dux with a password and `require = "everywhere"`, and a browser
/// on this machine (in dux's own network namespace, reaching it on loopback).
///
/// Task: the eavesdropping warning must appear only where it is true; traffic
/// that never leaves the machine cannot be overheard.
///
/// Action: open `http://127.0.0.1:<port>/` and read the login page.
///
/// Result: the login page shows and the plain-HTTP warning does not.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_12b_no_eavesdropping_warning_on_this_machine() {
    journey("12b-browser-loopback", Duration::from_secs(300), async {
        let dux = Dux::start(
            DuxOptions::local()
                .with_published(WEBDRIVER_PORT)
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.require", "everywhere"),
        )
        .await;
        let browser = Browser::beside(&dux).await;
        browser.goto(&format!("http://127.0.0.1:{DUX_PORT}/")).await;
        browser.wait_for(LOGIN_FORM, "the login page").await;
        assert!(
            !browser.is_shown(INSECURE_WARNING).await,
            "no eavesdropping warning for a page served over loopback"
        );
        browser.quit().await;
    })
    .await;
}

/// Situation: dux with NO password, listening on every interface, and a
/// browser on another machine of the same network.
///
/// Task: the web must warn, loudly, that anyone who can reach this page has
/// everything, and let the owner silence that for good.
///
/// Action: open dux; read the banner; press "don't show again"; reload.
///
/// Result: the red no-password banner is visible in the app; after "don't show
/// again" it is gone and stays gone across a reload, and config.toml now says
/// `disable_no_auth_warning = true`.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_12c_no_password_banner_until_told_never_again() {
    journey("12c-browser-banner", Duration::from_secs(300), async {
        let network = JourneyNetwork::create();
        // The first-run welcome and release-notes dialogs would sit over the
        // banner and take the click; a person dismisses them first.
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_network(&network)
                .with_config("ui.disable_automated_welcome_screen", "true")
                .with_config("ui.disable_release_notes", "true"),
        )
        .await;
        let browser = Browser::on_network(&network).await;
        let url = format!("http://{}:{DUX_PORT}/", dux.address().await);

        browser.goto(&url).await;
        browser
            .wait_for(NO_AUTH_BANNER, "the no-password banner")
            .await;
        browser
            .wait_for(NO_AUTH_NEVER, "the banner's don't-show-again control")
            .await
            .click()
            .await
            .expect("press don't show again");
        browser
            .wait_gone(NO_AUTH_BANNER, "the banner to go away")
            .await;
        eventually(
            "the setting to be written",
            Duration::from_secs(15),
            || async {
                dux.config_text()
                    .await
                    .contains("disable_no_auth_warning = true")
                    .then_some(())
            },
        )
        .await;

        browser.goto(&url).await;
        browser
            .wait_for("#root > *", "the app after a reload")
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(
            !browser.is_shown(NO_AUTH_BANNER).await,
            "the banner stays gone after a reload"
        );
        browser.quit().await;
    })
    .await;
}

/// Situation: dux whose password was set while the minimum strength score was
/// 0 and whose minimum was raised to the default afterwards, so the password
/// in force is below today's minimum; a browser on the network.
///
/// Task: dux can only judge a password when it sees it, which is at sign-in; it
/// must still let the owner in, and then tell them to change it.
///
/// Action: sign in with the weak password in the browser.
///
/// Result: signing in works, and the weak-password banner is visible in the app.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_12d_weak_password_banner_after_signing_in() {
    journey("12d-browser-weak", Duration::from_secs(300), async {
        let weak = "password1234";
        let network = JourneyNetwork::create();
        let dux = Dux::start(
            DuxOptions::exposed()
                .with_network(&network)
                .with_config("server.auth.minimum_password_score", "0")
                .with_password(weak)
                .with_config("server.auth.minimum_password_score", "2"),
        )
        .await;
        let browser = Browser::on_network(&network).await;
        browser
            .goto(&format!("http://{}:{DUX_PORT}/", dux.address().await))
            .await;
        sign_in(&browser, weak).await;
        browser
            .wait_gone(LOGIN_FORM, "the login form to go away")
            .await;
        browser
            .wait_for(WEAK_BANNER, "the weak-password banner")
            .await;
        browser.quit().await;
    })
    .await;
}
