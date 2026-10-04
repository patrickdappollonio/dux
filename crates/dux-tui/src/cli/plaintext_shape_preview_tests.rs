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
