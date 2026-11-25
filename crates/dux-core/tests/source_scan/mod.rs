//! Reads the workspace's Rust source for the guard tests in this directory.
//!
//! It reads text, line by line, and cuts a `//` comment from each line. A `//`
//! inside a string literal also cuts the line there, and a block comment is
//! read as code.

use std::path::{Path, PathBuf};

/// One line of source that matched, with the comment cut off.
#[derive(Debug)]
pub(crate) struct Hit {
    /// The file, relative to `crates/`.
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) code: String,
}

/// The workspace's `crates/` directory.
pub(crate) fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("dux-core sits inside crates/")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`, skipping build output and the browser UI.
pub(crate) fn rust_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&crates_dir(), &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("directory entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if !matches!(name, "target" | "node_modules" | "web") {
                walk(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The text of `line` before any `//` comment.
pub(crate) fn code_part(line: &str) -> &str {
    line.find("//").map_or(line, |at| &line[..at])
}

fn relative(path: &Path) -> String {
    path.strip_prefix(crates_dir())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Every line of every file whose code (comments cut) satisfies `matches`.
pub(crate) fn hits(files: &[PathBuf], matches: impl Fn(&str) -> bool) -> Vec<Hit> {
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file).expect("readable source");
        for (at, line) in text.lines().enumerate() {
            let code = code_part(line);
            if matches(code) {
                out.push(Hit {
                    file: relative(file),
                    line: at + 1,
                    code: code.trim().to_string(),
                });
            }
        }
    }
    out
}
