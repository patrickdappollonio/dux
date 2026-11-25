//! Every permission dux sets on a file is decided in
//! `crates/dux-core/src/file_modes.rs`: no production source outside it calls
//! `set_permissions`, builds a mode with `from_mode(`, or calls `chmod` (as a
//! function or as a command).
//!
//! Test code is exempt: files under a `tests/` directory, files declared as
//! `#[cfg(test)] mod name;`, and lines inside a `#[cfg(test)] mod name { .. }`.
//! It reads text: a mode set through another API (`OpenOptionsExt::mode`, a
//! `libc` call) is beyond it.

use std::collections::HashSet;
use std::path::PathBuf;

use source_scan::code_part;

mod source_scan;

/// The file that decides modes, relative to `crates/`.
const MODES_FILE: &str = "dux-core/src/file_modes.rs";

/// Production lines outside `file_modes.rs` that set a mode, each with how many
/// times its exact text may appear in its file and why it is allowed.
const ALLOWED: &[(&str, &str, usize, &str)] = &[
    (
        "dux-core/src/config_write.rs",
        "fs::set_permissions(tmp.path(), fs::Permissions::from_mode(CONFIG_FILE_MODE))",
        1,
        "the config's temporary file takes `file_modes::PRIVATE_FILE_MODE`, imported as \
         `CONFIG_FILE_MODE`, so the mode is still decided in file_modes.rs",
    ),
    (
        "dux-core/src/git.rs",
        "file.set_permissions(metadata.permissions())?;",
        1,
        "copying a file into a worktree gives the copy its source's own mode: user data, \
         not a file dux keeps for itself",
    ),
];

fn sets_a_mode(code: &str) -> bool {
    code.contains("set_permissions")
        || code.contains("from_mode(")
        || code.contains("chmod(")
        || code.contains("\"chmod\"")
}

/// Files that are compiled only for tests: those under a `tests/` directory and
/// those a parent declares as `#[cfg(test)] mod name;`.
fn test_only_files(files: &[PathBuf]) -> HashSet<PathBuf> {
    let mut out: HashSet<PathBuf> = files
        .iter()
        .filter(|f| f.components().any(|c| c.as_os_str() == "tests"))
        .cloned()
        .collect();
    for file in files {
        let text = std::fs::read_to_string(file).expect("readable source");
        let lines: Vec<&str> = text.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            let Some(name) = declared_module(code_part(line)) else {
                continue;
            };
            let above = &lines[at.saturating_sub(3)..at];
            if !above.iter().any(|l| code_part(l).contains("#[cfg(test)]")) {
                continue;
            }
            let dir = file.parent().expect("file has a directory");
            let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let base = if matches!(stem, "mod" | "lib" | "main") {
                dir.to_path_buf()
            } else {
                dir.join(stem)
            };
            out.insert(base.join(format!("{name}.rs")));
            out.insert(base.join(name).join("mod.rs"));
        }
    }
    out
}

/// The module name of a `mod name;` declaration.
fn declared_module(code: &str) -> Option<&str> {
    let rest = code.trim();
    let rest = rest
        .strip_prefix("pub(crate) ")
        .or_else(|| rest.strip_prefix("pub "))
        .unwrap_or(rest);
    let name = rest.strip_prefix("mod ")?.strip_suffix(';')?.trim();
    name.chars()
        .all(|c| c.is_alphanumeric() || c == '_')
        .then_some(name)
}

/// The 1-based line numbers of `text` that sit inside a `#[cfg(test)]` module.
fn test_module_lines(text: &str) -> HashSet<usize> {
    let mut inside = HashSet::new();
    let mut depth: i64 = 0;
    let mut module_depth: Option<i64> = None;
    let mut attribute_seen = false;
    for (at, line) in text.lines().enumerate() {
        let code = code_part(line);
        if code.contains("#[cfg(test)]") {
            attribute_seen = true;
        }
        if attribute_seen && module_depth.is_none() && opens_module(code) {
            module_depth = Some(depth);
            attribute_seen = false;
        }
        if module_depth.is_some() {
            inside.insert(at + 1);
        }
        depth += count(code, '{') - count(code, '}');
        if module_depth.is_some_and(|d| depth <= d) {
            module_depth = None;
        }
    }
    inside
}

fn opens_module(code: &str) -> bool {
    code.split_whitespace()
        .collect::<Vec<_>>()
        .windows(3)
        .any(|w| w[0] == "mod" && w[2].starts_with('{'))
        || code
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|w| w[0] == "mod" && w[1].ends_with('{') && w[1].len() > 1)
}

fn count(code: &str, brace: char) -> i64 {
    i64::try_from(code.chars().filter(|c| *c == brace).count()).expect("line length fits")
}

#[test]
fn a_test_module_is_found_by_its_attribute_and_braces() {
    let text = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() {\n    }\n}\nfn c() {}\n";
    let lines = test_module_lines(text);
    assert_eq!(lines, HashSet::from([3, 4, 5, 6]));
}

#[test]
fn only_file_modes_rs_sets_a_mode_outside_tests() {
    let files = source_scan::rust_files();
    let test_files = test_only_files(&files);
    let production: Vec<PathBuf> = files
        .into_iter()
        .filter(|f| !test_files.contains(f))
        .collect();
    let mut found = Vec::new();
    for hit in source_scan::hits(&production, sets_a_mode) {
        if hit.file == MODES_FILE {
            continue;
        }
        let text = std::fs::read_to_string(source_scan::crates_dir().join(&hit.file))
            .expect("readable source");
        if test_module_lines(&text).contains(&hit.line) {
            continue;
        }
        found.push(hit);
    }
    let mut unexplained = Vec::new();
    for hit in &found {
        let allowed = ALLOWED
            .iter()
            .find(|(file, line, _, _)| hit.file == *file && hit.code.starts_with(line));
        let Some((file, line, times, _)) = allowed else {
            unexplained.push(format!("  crates/{}:{}: {}", hit.file, hit.line, hit.code));
            continue;
        };
        let seen = found
            .iter()
            .filter(|h| h.file == *file && h.code.starts_with(line))
            .count();
        assert_eq!(
            seen, *times,
            "crates/{file} sets a mode with `{line}` {seen} times; it is allowed {times}"
        );
    }
    assert!(
        unexplained.is_empty(),
        "every mode dux sets is decided in crates/{MODES_FILE}; call it instead of \
         setting a mode here:\n{}",
        unexplained.join("\n")
    );
    for (file, line, _, _) in ALLOWED {
        assert!(
            found
                .iter()
                .any(|h| h.file == *file && h.code.starts_with(line)),
            "the allowance for crates/{file} `{line}` matches nothing: remove it"
        );
    }
}
