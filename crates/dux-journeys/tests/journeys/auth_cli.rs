//! The password from the command line, and every serving mode asking for it:
//! journey 10 of the login plan, and the three serving modes.

use std::time::Duration;

use dux_journeys::{Dux, DuxOptions, STRONG_PASSWORD, eventually, journey};
use serde_json::json;

use crate::auth_login::assert_auth_required;
use crate::smoke::start_web_server;

/// The comment lines of a config file, in order.
fn comments(config: &str) -> Vec<String> {
    config
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The `password_hash` value in a config file, if one is set.
fn password_hash(config: &str) -> Option<String> {
    config
        .lines()
        .find_map(|l| l.trim().strip_prefix("password_hash = "))
        .map(|v| v.trim_matches('"').to_string())
        .filter(|v| !v.is_empty())
}

/// Situation: a running `dux server` with no password, reachable from the
/// network, its config.toml the fully commented canonical file.
///
/// Task: the owner sets the web password from a terminal with
/// `dux config set server.auth.password`, which must refuse an unsafe password,
/// never take one on the command line, keep every comment, and reach the
/// running dux.
///
/// Action: pipe a long but common password to `--stdin`; pipe a short one; pass
/// a strong one as an argument; misspell the key; pipe a strong one; read the
/// file; `dux config get` the hash and the password key; read the auth status
/// from the network.
///
/// Result: the common password is refused for its strength score and the short
/// one for the minimum length (12), each with a non-zero exit, a message saying
/// why, and the file untouched; the argument form is refused with a pointer to
/// `--stdin` and writes nothing; the misspelled key is refused with a suggestion
/// naming `server.auth.password`; the strong one exits 0 and writes
/// `password_hash` as an Argon2id PHC string, never the password, with every
/// comment line of the file still there in order; `get` prints that hash for
/// both keys; no command ever printed the password; and the running dux
/// reports a password set, without a restart.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_10_config_set_password_from_stdin_keeps_comments_and_refuses_weak_ones() {
    journey("10-config-set-password", Duration::from_secs(180), async {
        let dux = Dux::start(DuxOptions::exposed()).await;
        let before = dux.config_text().await;
        assert!(
            password_hash(&before).is_none(),
            "a fresh dux has no password"
        );

        let weak = dux
            .config_set_secret("server.auth.password", "password1234")
            .await;
        assert_ne!(
            weak.code,
            0,
            "a common password is refused: {}",
            weak.output()
        );
        let said = weak.output().to_ascii_lowercase();
        assert!(
            said.contains("weak") || said.contains("score") || said.contains("strength"),
            "the refusal says the password is too weak: {said}"
        );

        let short = dux
            .config_set_secret("server.auth.password", "Qz7#vL9p")
            .await;
        assert_ne!(
            short.code,
            0,
            "a short password is refused: {}",
            short.output()
        );
        assert!(
            short.output().contains("12"),
            "the refusal names the minimum length: {}",
            short.output()
        );

        let argument = dux
            .config_set("server.auth.password", STRONG_PASSWORD)
            .await;
        assert_ne!(
            argument.code, 0,
            "a password on the command line is refused"
        );
        assert!(
            argument.output().contains("--stdin"),
            "the refusal points at --stdin: {}",
            argument.output()
        );

        let typo = dux.config_set("server.auth.pasword", "x").await;
        assert_ne!(typo.code, 0);
        assert!(
            typo.output().contains("server.auth.password"),
            "an unknown key gets a suggestion: {}",
            typo.output()
        );

        assert_eq!(
            dux.config_text().await,
            before,
            "nothing refused changed the file"
        );

        let set = dux
            .config_set_secret("server.auth.password", STRONG_PASSWORD)
            .await;
        assert_eq!(set.code, 0, "{}", set.output());

        let after = dux.config_text().await;
        let hash = password_hash(&after).expect("a password_hash is written");
        assert!(
            hash.starts_with("$argon2id$v=19$"),
            "an Argon2id PHC string: {hash}"
        );
        assert!(
            !after.contains(STRONG_PASSWORD),
            "the password is never written"
        );
        let (old, new) = (comments(&before), comments(&after));
        let mut remaining = new.iter();
        for line in &old {
            assert!(
                remaining.any(|l| l == line),
                "the comment {line:?} survived the write, in order"
            );
        }

        for key in ["server.auth.password_hash", "server.auth.password"] {
            let got = dux.config_get(key).await;
            assert_eq!(got.code, 0, "{}", got.output());
            assert_eq!(
                got.stdout.trim().trim_matches('"'),
                hash,
                "dux config get {key}"
            );
        }

        for run in [&weak, &short, &argument, &typo, &set] {
            assert!(
                !run.output().contains(STRONG_PASSWORD) && !run.output().contains("password1234"),
                "no command prints a password: {}",
                run.output()
            );
        }

        let client = dux.client().await;
        eventually(
            "the running dux to learn the password",
            Duration::from_secs(20),
            || async { (client.auth_status().await["password_set"] == json!(true)).then_some(()) },
        )
        .await;
        assert_auth_required(
            &client.get("/api/v1/projects").await,
            "the network, once a password is set",
        );
    })
    .await;
}

/// Situation: the same password and `require = "everywhere"` in three
/// containers, one per serving mode: `dux server`, the terminal UI with
/// `[server] serve_while_tui = true`, and the terminal UI flipped to a server
/// with the `start-web-server` palette command. Each has a port relaying onto
/// loopback (the two terminal UI modes bind loopback only).
///
/// Task: the password is a property of serving, not of one mode; every way dux
/// serves must ask for it.
///
/// Action: in each container, list projects with no session, sign in with the
/// wrong password and with the right one, and list again.
///
/// Result: in all three, the first list is refused with 401 `auth_required`, the
/// wrong password with 401, the right one signs in with 204, and the list then
/// answers.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(feature = "auth"),
    ignore = "waits on the web password login; run with --features auth"
)]
async fn journey_every_serving_mode_asks_for_the_password() {
    journey("serving-modes", Duration::from_secs(300), async {
        let base = || {
            DuxOptions::local()
                .with_loopback_relay(4200)
                .with_password(STRONG_PASSWORD)
                .with_config("server.auth.require", "everywhere")
        };
        let server = Dux::start(base()).await;
        let background = Dux::start(
            base()
                .with_tui()
                .with_config("server.serve_while_tui", "true"),
        )
        .await;
        background.wait_healthy().await;
        let flip = Dux::start(base().with_tui()).await;
        start_web_server(&flip).await;
        flip.wait_healthy().await;

        for (mode, dux) in [
            ("dux server", &server),
            ("serve_while_tui", &background),
            ("start-web-server", &flip),
        ] {
            let client = dux.client_on(4200).await;
            assert_auth_required(&client.get("/api/v1/projects").await, mode);
            let wrong = client.login("not-the-password-at-all").await;
            assert_eq!(wrong.status, 401, "{mode}: {}", wrong.describe());
            client.login_ok(STRONG_PASSWORD).await;
            let listed = client.get("/api/v1/projects").await;
            assert_eq!(listed.status, 200, "{mode}: {}", listed.describe());
        }
    })
    .await;
}
