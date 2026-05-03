//! Integration test: verify `logger::init` writes JSON Lines records to a
//! daily-rotated file, and that both `tracing::*!` macros and the legacy
//! `crate::logger::*` shims funnel into the same file.
//!
//! This is the contract Phase 09 of audit02 introduces; without it,
//! Phase 10 (GDPR purge) and Phase 20 (doctor tool) cannot rely on
//! parseable log output.

use std::path::PathBuf;

use dux::config::{DuxPaths, LoggingConfig};

fn fake_paths(root: PathBuf) -> DuxPaths {
    DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        root,
    }
}

#[test]
fn init_emits_json_lines_with_target_and_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = fake_paths(dir.path().to_path_buf());
    let cfg = LoggingConfig {
        level: "info".to_string(),
        path: String::new(),
    };

    dux::logger::init(&cfg, &paths);

    tracing::info!(
        target: "dux::probe",
        session_id = "demo",
        n = 3i64,
        "hello world",
    );
    // Legacy shim: must also reach the file, sanitized.
    dux::logger::error("legacy with \x1b[31mansi\x1b[0m bytes");

    // Flush by giving the non-blocking writer a moment.
    std::thread::sleep(std::time::Duration::from_millis(300));

    // Find the rotated file. The appender writes `dux.log.YYYY-MM-DD`.
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("dux.log"))
        })
        .collect();
    assert!(
        !entries.is_empty(),
        "no dux.log* file produced under {}",
        dir.path().display()
    );

    let mut all_text = String::new();
    for entry in &entries {
        all_text.push_str(&std::fs::read_to_string(entry).expect("read log file"));
    }
    assert!(!all_text.is_empty(), "log file is empty");

    // Each non-empty line must parse as JSON.
    let mut saw_probe = false;
    let mut saw_legacy_sanitized = false;
    for line in all_text.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("non-JSON line: {line:?} ({e})"));
        let target = parsed.get("target").and_then(|v| v.as_str()).unwrap_or("");
        if target == "dux::probe" {
            saw_probe = true;
            // Structured field must be present.
            let fields = parsed.get("fields").expect("fields object");
            assert_eq!(
                fields.get("session_id").and_then(|v| v.as_str()),
                Some("demo")
            );
            assert_eq!(fields.get("n").and_then(|v| v.as_i64()), Some(3));
        }
        if target == "dux::legacy" {
            saw_legacy_sanitized = true;
            let msg = parsed
                .get("fields")
                .and_then(|f| f.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("");
            // The raw ESC byte must have been rewritten by the sanitizer.
            assert!(
                !msg.contains('\u{1b}'),
                "legacy shim leaked ESC bytes into log: {msg}"
            );
            assert!(
                msg.contains("\\x1b"),
                "sanitizer should rewrite ESC as \\xNN; got {msg}"
            );
        }
    }
    assert!(saw_probe, "expected at least one dux::probe record");
    assert!(saw_legacy_sanitized, "expected legacy shim record");
}
