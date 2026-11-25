//! dux targets macOS and Linux only, so no Rust source in the workspace holds a
//! Windows-only or not-Unix code path.
//!
//! It reads text with comments cut, so a condition spelled another way (a
//! `cfg_attr`, a `target_family` check, a macro that builds the condition) is
//! beyond it.

mod source_scan;

/// The conditions that select a Windows or non-Unix build. Each is spelled in
/// two pieces so this file does not match itself.
const WINDOWS_CONDITIONS: [&str; 4] = [
    concat!("cfg(", "windows)"),
    concat!("cfg!(", "windows)"),
    concat!("target_os = ", "\"windows\""),
    concat!("cfg(not(", "unix))"),
];

fn names_windows(code: &str) -> bool {
    WINDOWS_CONDITIONS.iter().any(|c| code.contains(c))
}

#[test]
fn no_rust_source_has_a_windows_or_non_unix_code_path() {
    let hits = source_scan::hits(&source_scan::rust_files(), names_windows);
    assert!(
        hits.is_empty(),
        "dux targets macOS and Linux only (Windows users run it in WSL2), so no code is \
         conditional on Windows or on not being Unix:\n{}",
        hits.iter()
            .map(|h| format!("  crates/{}:{}: {}", h.file, h.line, h.code))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_detector_matches_each_condition_and_ignores_comments() {
    for condition in WINDOWS_CONDITIONS {
        assert!(names_windows(&format!("#[{condition}]")), "{condition}");
        assert!(
            !names_windows(source_scan::code_part(&format!("// no {condition} here"))),
            "{condition} in a comment"
        );
    }
    assert!(!names_windows("#[cfg(unix)]"));
}
