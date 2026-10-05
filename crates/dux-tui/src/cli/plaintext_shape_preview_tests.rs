use super::*;

const S: &str = "PLAINSECRETzz9q";

/// A plaintext password, by dux's own start check, is printed by the
/// regenerate preview with --show.
#[test]
fn regenerate_preview_with_show_never_prints_a_plaintext_password() {
    crate::config::install_canonical_renderer();
    let fresh = crate::config::render_default_config();
    let mut leaks = Vec::new();
    for body in [
        format!("[[env]]\npassword = \"{S}\"\n"),
        format!("[projects.env]\npassword = \"{S}\"\n"),
        format!("\"env.password\" = \"{S}\"\n"),
        format!("[server]\n\"auth.password\" = \"{S}\"\n"),
    ] {
        assert!(
            !dux_core::config::plaintext_password_problems(&body).is_empty(),
            "dux does not call {body:?} a plaintext password"
        );
        let out = regenerate_preview(&body, &fresh, true);
        if out.contains(S) {
            leaks.push(format!("{body:?}:\n{out}"));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n----\n"));
}

/// A file that is not TOML: a plaintext password line the fallback does
/// not recognise is printed with --show.
#[test]
fn regenerate_preview_of_a_broken_file_with_show_never_prints_a_password() {
    crate::config::install_canonical_renderer();
    let fresh = crate::config::render_default_config();
    let mut leaks = Vec::new();
    for body in [
        format!("server.auth.password = \"{S}\"\nbroken =\n"),
        format!("[server.auth]\n'password' = \"{S}\"\nbroken =\n"),
        format!("[server.auth]\npassword = \"\"\"\n{S}\n\"\"\"\nbroken =\n"),
    ] {
        let out = regenerate_preview(&body, &fresh, true);
        if out.contains(S) {
            leaks.push(format!("{body:?}:\n{out}"));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n----\n"));
}

/// Without --show, nothing in [env] or a project's entry (comments
/// included) is printed by either preview.
#[test]
fn previews_without_show_never_print_env_or_project_text() {
    crate::config::install_canonical_renderer();
    let fresh = crate::config::render_default_config();
    let mut leaks = Vec::new();
    for body in [
        format!("[env] # {S}\nA = \"v\"\n"),
        format!(
            "[[projects]] # {S}
path = \"/tmp/x\"
"
        ),
    ] {
        let regen = regenerate_preview(&body, &fresh, false);
        if regen.contains(S) {
            leaks.push(format!("regenerate {body:?}:\n{regen}"));
        }
        if let Ok(restored) = crate::config::restore_documentation(&body) {
            let out = restore_docs_preview(&body, &restored.text, false);
            if out.contains(S) {
                let lines: Vec<&str> = out.lines().filter(|l| l.contains(S)).collect();
                leaks.push(format!("restore-docs {body:?}:\n{}", lines.join("\n")));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n----\n"));
}

/// The password hash and the blocked addresses are held back by both
/// previews without `--show`, as `dux config diff` holds them back, in every
/// way the file can write them; `--show` shows them.
#[test]
fn previews_hold_back_the_password_hash_and_blocked_addresses_without_show() {
    crate::config::install_canonical_renderer();
    let fresh = crate::config::render_default_config();
    let hash = dux_core::auth::hash_password(&dux_core::auth::Password::new(
        "correct horse battery staple veranda".to_string(),
    ))
    .expect("hash");
    let address = "203.0.113.77";
    let mut wrong = Vec::new();
    for body in [
        format!("[server.auth]\npassword_hash = \"{hash}\"\nblocked_addresses = [\"{address}\"]\n"),
        format!(
            "[server]\nauth = {{ password_hash = \"{hash}\", blocked_addresses = [\"{address}\"] }}\n"
        ),
        format!(
            "server.auth.password_hash = \"{hash}\"\nserver.auth.blocked_addresses = [\n  \"{address}\",\n]\n"
        ),
    ] {
        let mut hidden = vec![("regenerate", regenerate_preview(&body, &fresh, false))];
        let mut shown = vec![("regenerate", regenerate_preview(&body, &fresh, true))];
        if let Ok(restored) = crate::config::restore_documentation(&body) {
            hidden.push((
                "restore-docs",
                restore_docs_preview(&body, &restored.text, false),
            ));
            shown.push((
                "restore-docs",
                restore_docs_preview(&body, &restored.text, true),
            ));
        }
        for (preview, out) in hidden {
            for secret in [hash.as_str(), address] {
                if out.contains(secret) {
                    wrong.push(format!(
                        "{preview} without --show prints {secret} for {body:?}:\n{out}"
                    ));
                }
            }
        }
        let (_, regenerate_shown) = &shown[0];
        for secret in [hash.as_str(), address] {
            if !regenerate_shown.contains(secret) {
                wrong.push(format!(
                    "regenerate --show leaves out {secret} for {body:?}"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n----\n"));
}
