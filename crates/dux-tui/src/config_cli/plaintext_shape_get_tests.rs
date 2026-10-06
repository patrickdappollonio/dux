use super::*;

const S: &str = "PLAINSECRETzz9q";

fn paths_in(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}
fn get_all(body: &str, args: &[&str]) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, body).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_get(&args, &paths, &mut out, &mut err);
    format!(
        "{r:?}\nOUT:{}\nERR:{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}

/// dux itself calls each of these a plaintext password (it stops the start
/// for that reason), yet `get --show` prints it.
#[test]
fn get_show_never_prints_a_plaintext_password_dux_itself_flags() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for (body, key) in [
        (format!("[projects.env]\npassword = \"{S}\"\n"), "projects"),
        (format!("[[env]]\npassword = \"{S}\"\n"), "env"),
        (format!("[server]\n\"auth.password\" = \"{S}\"\n"), "server"),
        (
            format!("[[projects]]\npath = \"/tmp/p\"\npassword = \"{S}\"\n"),
            "projects",
        ),
    ] {
        assert!(
            !dux_core::config::plaintext_password_problems(&body).is_empty(),
            "dux does not call {body:?} a plaintext password"
        );
        let said = get_all(&body, &[key, "--show"]);
        if said.contains(S) {
            leaks.push(format!("get {key} --show on {body:?}:\n{said}"));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n----\n"));
}
