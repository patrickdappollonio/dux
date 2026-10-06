//! `dux macros`, `providers`, `keys`, `themes` and `env` against a real dux:
//! one served in this process on a temporary config folder, reached by the
//! built binary over the control socket and, as a saved remote, over HTTP.
//! Then the same binary with no dux running, editing the file itself.
//!
//! A test binary of its own, because the dux it starts runs until this
//! process ends.

mod support;

use std::path::PathBuf;

use support::{dux, serve};

/// An empty, owner-only config folder.
fn home(name: &str) -> PathBuf {
    support::home("cfgres", name)
}

const CONFIG: &str = "# kept by hand\n[macros]\n# says hi\nhi = { text = \"hello\", surface = \"agent\" }\n\n[ui]\ngithub_integration = false\n\n[server]\ntailscale = \"no\"\n";

#[test]
fn config_file_resources_through_a_running_dux_and_with_none() {
    let root = home("running");
    std::fs::write(root.join("config.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(root.join("themes/alpha.toml"), "").unwrap();
    // Nothing the serve might run (gh, tailscale, git) is the developer's own.
    // SAFETY: this binary runs this one test, and no other thread is running
    // yet.
    unsafe { std::env::set_var("PATH", root.join("no-tools")) };
    let port = serve(&root);
    let run = dux(
        &root,
        &["remote", "add", "here", &format!("http://127.0.0.1:{port}")],
        "",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());

    // A macro saved with dux running is in the running dux at once, which
    // the remote's API answers from, and in the file, comments kept.
    let run = dux(
        &root,
        &[
            "macros",
            "add",
            "greet",
            "hello there",
            "--surface",
            "both",
            "--yes",
        ],
        "",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(run.stdout().contains("greet"), "{}", run.stdout());
    let run = dux(&root, &["--remote", "here", "macros", "ls"], "");
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert_eq!(
        run.stdout(),
        "NAME    SURFACE   TEXT\nhi      agent     hello\ngreet   both      hello there\n"
    );
    let file = std::fs::read_to_string(root.join("config.toml")).unwrap();
    assert!(
        file.contains(
            "# kept by hand\n[macros]\n# says hi\nhi = { text = \"hello\", surface = \"agent\" }\n\
             greet = { text = \"hello there\", surface = \"both\" }\n"
        ),
        "{file}"
    );

    // Removing one that is not there is the running dux's refusal.
    let run = dux(&root, &["macros", "rm", "nope", "--yes"], "");
    assert_eq!(run.code(), 1, "{}", run.stderr());
    assert!(run.stderr().contains("nope"), "{}", run.stderr());

    // A value goes in through standard input and is never printed back.
    let run = dux(
        &root,
        &["env", "set", "API_TOKEN", "--stdin", "--yes"],
        "zzsecret\n",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(!run.stdout().contains("zzsecret"), "{}", run.stdout());
    let run = dux(&root, &["--remote", "here", "env", "ls"], "");
    assert_eq!(run.stdout(), "NAME\nAPI_TOKEN\n");
    let run = dux(&root, &["--remote", "here", "env", "ls", "--show"], "");
    assert_eq!(run.stdout(), "NAME        VALUE\nAPI_TOKEN   zzsecret\n");

    // The themes are the picker's, in its order, here and through the remote.
    let here = dux(&root, &["themes", "ls", "-q"], "");
    assert_eq!(here.code(), 0, "{}", here.stderr());
    assert!(
        here.stdout().starts_with("dux_dark\nalpha\nayu_dark\n"),
        "{}",
        here.stdout()
    );
    let remote = dux(&root, &["--remote", "here", "themes", "ls", "-q"], "");
    assert_eq!(remote.code(), 0, "{}", remote.stderr());
    assert_eq!(remote.stdout(), here.stdout());

    for listing in [&["keys", "ls", "-q"][..], &["providers", "ls", "-q"][..]] {
        let here = dux(&root, listing, "");
        let mut through = vec!["--remote", "here"];
        through.extend_from_slice(listing);
        let remote = dux(&root, &through, "");
        assert_eq!(here.code(), 0, "{listing:?}: {}", here.stderr());
        assert_eq!(remote.code(), 0, "{listing:?}: {}", remote.stderr());
        assert!(!here.stdout().is_empty(), "{listing:?}");
        assert_eq!(remote.stdout(), here.stdout(), "{listing:?}");
    }

    // With no dux running the command edits the file, comments kept.
    let stopped = home("stopped");
    std::fs::write(stopped.join("config.toml"), CONFIG).unwrap();
    let run = dux(&stopped, &["macros", "add", "bye", "see you", "--yes"], "");
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(
        run.stdout().contains("applies the next time dux starts"),
        "{}",
        run.stdout()
    );
    assert_eq!(
        std::fs::read_to_string(stopped.join("config.toml")).unwrap(),
        CONFIG.replace(
            "surface = \"agent\" }\n",
            "surface = \"agent\" }\nbye = { text = \"see you\", surface = \"agent\" }\n"
        )
    );
    let run = dux(&stopped, &["macros", "ls", "-q"], "");
    assert_eq!(run.stdout(), "hi\nbye\n");
    assert!(!stopped.join("sessions.sqlite3").exists());

    let _ = std::fs::remove_dir_all(&stopped);
    let _ = std::fs::remove_dir_all(&root);
}
