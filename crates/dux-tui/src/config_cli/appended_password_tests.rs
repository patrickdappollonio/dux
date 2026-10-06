//! A password line a user adds to the documented config.toml dux writes:
//! at its very end, and inside its `[macros]`, the user-named map it used to
//! end in. A string can never be a macro, so neither is a macro's name.
use super::*;

fn paths(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}

/// dux's own documented file with `line` appended at its end.
fn appended(line: &str) -> String {
    crate::config::install_canonical_renderer();
    format!("{}\n{line}\n", crate::config::render_default_config())
}

/// dux's own documented file with `line` written first inside `[section]`.
fn inside(section: &str, line: &str) -> String {
    crate::config::install_canonical_renderer();
    let base = crate::config::render_default_config();
    let header = format!("\n[{section}]\n");
    let at = base
        .find(&header)
        .expect("the documented file has the section")
        + header.len();
    format!("{}{line}\n{}", &base[..at], &base[at..])
}

/// The documented file never ends in a map of user-chosen names, where an
/// appended setting would silently become one of its entries.
#[test]
fn the_documented_file_does_not_end_in_a_map_of_names() {
    crate::config::install_canonical_renderer();
    let base = crate::config::render_default_config();
    let last = base
        .lines()
        .rfind(|line| line.starts_with('['))
        .expect("a section");
    for map in ["[env]", "[macros]", "[keys]", "[providers]", "[[projects]]"] {
        assert!(
            !last.starts_with(map.trim_end_matches(']')),
            "the documented file ends in {last}"
        );
    }
}

/// A password hash added at the end of the documented file, or inside its
/// `[macros]`, never lets `dux server` start without it.
#[test]
fn a_password_hash_added_to_the_documented_file_never_starts_dux_server_without_it() {
    let hash = dux_core::auth::hash_password(&dux_core::auth::Password::new(
        "correct horse battery staple veranda".to_string(),
    ))
    .expect("hash");
    let line = format!("password_hash = \"{hash}\"");
    for body in [
        appended(&line),
        inside("macros", &line),
        inside("keys", &line),
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = paths(tmp.path());
        std::fs::write(&p.config_path, &body).expect("seed");
        if let Ok(config) = dux_core::config::load_config(&p) {
            assert!(
                config.server.auth.has_password(),
                "dux server starts with no password from:\n{}",
                &body[body.len().saturating_sub(300)..]
            );
        }
    }
}

/// A plaintext password added at the end of the documented file, or inside
/// its `[macros]`, is found by the start check and printed by nothing.
#[test]
fn a_plaintext_password_added_to_the_documented_file_is_found_and_never_printed() {
    let secret = "AppendedSekret42";
    let line = format!("password = \"{secret}\"");
    for (place, body) in [
        ("end", appended(&line)),
        ("macros", inside("macros", &line)),
        ("keys", inside("keys", &line)),
    ] {
        let tmp = tempfile::tempdir().expect("tempdir");
        let p = paths(tmp.path());
        std::fs::write(&p.config_path, &body).expect("seed");
        let mut said = String::new();
        for table in ["macros", "keys", "server", "server.auth"] {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let _ = run_get(&[table.to_string()], &p, &mut out, &mut err);
            said.push_str(&String::from_utf8_lossy(&out));
            said.push_str(&String::from_utf8_lossy(&err));
        }
        let preview =
            crate::cli::restore_docs_preview(&body, &crate::config::render_default_config(), false);
        let mut wrong = Vec::new();
        if dux_core::config::plaintext_password_problems(&body).is_empty() {
            wrong.push("the start check does not call it a plaintext password".to_string());
        }
        if dux_core::config::load_config(&p).is_ok() {
            wrong.push("dux server starts with it".to_string());
        }
        if said.contains(secret) {
            wrong.push(format!("`dux config get` prints it:\n{said}"));
        }
        if preview.contains(secret) {
            wrong.push("`dux config restore-docs` previews it".to_string());
        }
        assert!(wrong.is_empty(), "added at {place}: {}", wrong.join("\n"));
    }
}

/// Every `[server.auth]` setting, added at the end of the documented file
/// or inside its `[macros]`, as a plaintext string and as a password hash:
/// wherever it lands, dux either reads it in `[server.auth]` or refuses to
/// start, and a plaintext `password` is always one the start check names.
#[test]
fn every_auth_setting_added_to_the_documented_file_is_read_or_stops_the_start() {
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";
    let mut names = dux_core::config_auth::auth_setting_names();
    names.extend(["password_hash".to_string(), "password".to_string()]);
    names.sort();
    names.dedup();
    let mut wrong = Vec::new();
    for name in &names {
        for value in ["AppendedSekret42", hash] {
            let line = format!("{name} = \"{value}\"");
            for (place, body) in [
                ("end", appended(&line)),
                ("macros", inside("macros", &line)),
                ("keys", inside("keys", &line)),
            ] {
                let Ok(file) = toml::from_str::<toml::Table>(&body) else {
                    // A key written twice: not TOML, which stops the start.
                    continue;
                };
                let in_auth = file
                    .get("server")
                    .and_then(|server| server.get("auth"))
                    .and_then(|auth| auth.get(name))
                    .is_some_and(|written| written.as_str() == Some(value));
                let tmp = tempfile::tempdir().expect("tempdir");
                let p = paths(tmp.path());
                std::fs::write(&p.config_path, &body).expect("seed");
                let starts = dux_core::config::load_config(&p).is_ok();
                if !in_auth && starts {
                    wrong.push(format!("{line} at {place}: dux server starts without it"));
                }
                if name == "password"
                    && dux_core::config::plaintext_password_problems(&body).is_empty()
                {
                    wrong.push(format!(
                        "{line} at {place}: the start check does not name it"
                    ));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
