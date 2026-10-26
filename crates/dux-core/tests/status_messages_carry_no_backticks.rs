//! A status sentence never carries a literal backtick.
//!
//! The terminal UI's status line prints a status's plain message and the web's
//! toasts draw it, and neither renders Markdown: a command written as
//! `` `dux config set ...` `` shows its backticks verbatim on both. A command, a
//! setting or a path in a status is a name, built with `status_text!` (`q(..)`
//! or `n(..)`), so the web draws it as a chip and the plain spelling quotes it.
//!
//! This reads the production source of the three crates that build statuses
//! (dux-core, dux-web, dux-tui) and fails on every string literal holding a
//! backtick that is not accounted for. A literal is accounted for when:
//!
//! - its statement only logs, prints to the terminal dux was started from,
//!   panics or documents (`logger::`, `println!`, `eprintln!`, `panic!`,
//!   `assert`, `.expect(`, `unreachable!`, `#[doc`): none of those is a status;
//! - a `// backtick-ok: <reason>` comment sits in its statement or on the lines
//!   just above it, saying where the text goes instead;
//! - its whole file is in [`ALLOWED_FILES`], with the reason.
//!
//! What it cannot see: a backtick that reaches a status from text dux did not
//! write (a provider's output, a `git` or `gh` error, a TOML parse error); a
//! backtick assembled at runtime rather than written in a literal; and a
//! marked or allowlisted literal that later starts reaching a status. It reads
//! text and does not follow values, so the marker's reason is what review
//! checks.

use std::path::{Path, PathBuf};

/// Whole files whose literals never reach a status, each with the reason.
const ALLOWED_FILES: &[(&str, &str)] = &[
    (
        "dux-tui/src/config.rs",
        "the canonical config template: its backticks are comments in config.toml",
    ),
    (
        "dux-core/src/welcome.rs",
        "welcome tips: both surfaces render a backticked span as a highlight and hide the backticks",
    ),
    (
        "dux-tui/src/cli.rs",
        "the command line's own output, printed to the terminal it runs in",
    ),
    (
        "dux-tui/src/config_cli.rs",
        "the command line's own output, printed to the terminal it runs in",
    ),
    (
        "dux-core/src/release_notes.rs",
        "the release-notes Markdown reader: backticks are the syntax it strips",
    ),
];

/// Calls whose message is never a status, and the doc attribute.
const NOT_A_STATUS: &[&str] = &[
    "#[doc",
    "logger::",
    "println!",
    "eprintln!",
    "panic!",
    "assert",
    ".expect(",
    "unreachable!",
];

const MARKER: &str = "backtick-ok:";

/// One string literal: where it starts and ends (byte offsets, quotes
/// included) and whether its text holds a backtick.
struct Literal {
    start: usize,
    end: usize,
    backtick: bool,
}

/// The source with every comment and literal blanked to spaces (newlines
/// kept, same byte length), and the string literals found on the way.
fn lex(source: &str) -> (Vec<u8>, Vec<Literal>) {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut literals = Vec::new();
    let blank = |code: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut code[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        let next = bytes.get(i + 1).copied();
        match bytes[i] {
            b'/' if next == Some(b'/') => {
                let end = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
                blank(&mut code, i, end);
                i = end;
            }
            b'/' if next == Some(b'*') => {
                let mut depth = 0usize;
                let mut j = i;
                while j < bytes.len() {
                    if bytes[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if bytes[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                blank(&mut code, i, j);
                i = j;
            }
            b'r' | b'b' if i == 0 || !ident(bytes[i - 1]) => {
                // b"..", r"..", r#".."#, br#".."#
                let mut j = i + 1;
                if bytes[i] == b'b' && bytes.get(j) == Some(&b'r') {
                    j += 1;
                }
                let raw = bytes[i] == b'r' || j == i + 2;
                let hashes = bytes[j..].iter().take_while(|&&b| b == b'#').count();
                if bytes.get(j + hashes) == Some(&b'"') && (raw || hashes == 0) {
                    let body = j + hashes + 1;
                    let end = if raw {
                        let close: Vec<u8> = std::iter::once(b'"')
                            .chain(std::iter::repeat_n(b'#', hashes))
                            .collect();
                        find(bytes, body, &close).map_or(bytes.len(), |n| n + close.len())
                    } else {
                        string_end(bytes, body)
                    };
                    literals.push(Literal {
                        start: i,
                        end,
                        backtick: bytes[body..end].contains(&b'`'),
                    });
                    blank(&mut code, i, end);
                    i = end;
                } else {
                    i += 1;
                }
            }
            b'"' => {
                let end = string_end(bytes, i + 1);
                literals.push(Literal {
                    start: i,
                    end,
                    backtick: bytes[i + 1..end].contains(&b'`'),
                });
                blank(&mut code, i, end);
                i = end;
            }
            b'\'' => {
                // A char literal ('x', '\n', '\u{..}', a multi-byte char) or a
                // lifetime ('a), which has no closing quote.
                let end = if next == Some(b'\\') {
                    source[i + 2..]
                        .find('\'')
                        .map(|n| i + 2 + n + 1)
                        .unwrap_or(bytes.len())
                } else {
                    let width = source[i + 1..].chars().next().map_or(1, char::len_utf8);
                    if bytes.get(i + 1 + width) == Some(&b'\'') {
                        i + 2 + width
                    } else {
                        i + 1
                    }
                };
                blank(&mut code, i, end);
                i = end.max(i + 1);
            }
            _ => i += 1,
        }
    }
    (code, literals)
}

/// Where a plain (escaping) string whose body starts at `from` ends, past its
/// closing quote.
fn string_end(bytes: &[u8], from: usize) -> usize {
    let mut j = from;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|n| from + n)
}

/// The byte ranges of every item marked `#[cfg(test)]` (an inline test
/// module, a test-only helper): from the attribute to the end of the item, a
/// `;` or the brace that closes its body.
fn test_items(code: &[u8]) -> Vec<(usize, usize)> {
    let attribute = b"#[cfg(test)]";
    let mut items = Vec::new();
    let mut from = 0;
    while let Some(at) = find(code, from, attribute) {
        let mut depth = 0usize;
        let mut end = code.len();
        for (n, &b) in code.iter().enumerate().skip(at + attribute.len()) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = n + 1;
                        break;
                    }
                }
                b';' if depth == 0 => {
                    end = n + 1;
                    break;
                }
                _ => {}
            }
        }
        items.push((at, end));
        from = end;
    }
    items
}

/// The 1-based lines of every literal holding a backtick that is not
/// accounted for.
fn unaccounted(source: &str) -> Vec<usize> {
    let (code, literals) = lex(source);
    let tests = test_items(&code);
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let mut found = Vec::new();
    for literal in literals.iter().filter(|literal| literal.backtick) {
        if tests
            .iter()
            .any(|&(from, to)| (from..to).contains(&literal.start))
        {
            continue;
        }
        // The statement runs back to the nearest `;`, `{` or `}` in code; the
        // comments between that and the statement belong to it.
        let start = code[..literal.start]
            .iter()
            .rposition(|&b| matches!(b, b';' | b'{' | b'}'))
            .map_or(0, |n| n + 1);
        let statement_code = String::from_utf8_lossy(&code[start..literal.end]);
        if NOT_A_STATUS
            .iter()
            .any(|call| statement_code.contains(call))
        {
            continue;
        }
        // The rest of the statement, to its end, for a marker written after
        // the literal (inside a `match` arm, say).
        let rest_end = code[literal.end..]
            .iter()
            .position(|&b| matches!(b, b';' | b'{' | b'}'))
            .map_or(source.len(), |n| literal.end + n);
        if source[start..rest_end].contains(MARKER) {
            continue;
        }
        found.push(line_of(literal.start));
    }
    found
}

/// The files of `src` that only a `#[cfg(test)] mod name;` declares, and
/// everything below them: test code, which builds no status anyone sees.
fn test_only_files(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut test_only = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(file).expect("read source");
        let lines: Vec<&str> = source.lines().collect();
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let dir = if matches!(stem, "mod" | "lib" | "main") {
            file.parent().unwrap().to_path_buf()
        } else {
            file.with_extension("")
        };
        for (n, line) in lines.iter().enumerate() {
            if line.trim() != "#[cfg(test)]" {
                continue;
            }
            let mut path_attr = None;
            for next in lines.iter().skip(n + 1).take(4) {
                let next = next.trim();
                if let Some(rest) = next.strip_prefix("#[path = \"") {
                    path_attr = rest.strip_suffix("\"]").map(str::to_string);
                    continue;
                }
                if next.starts_with("#[") {
                    continue;
                }
                let declared = next
                    .trim_start_matches("pub(crate) ")
                    .trim_start_matches("pub ")
                    .strip_prefix("mod ")
                    .and_then(|rest| rest.strip_suffix(';'));
                if let Some(name) = declared {
                    match &path_attr {
                        Some(path) => test_only.push(file.parent().unwrap().join(path)),
                        None => {
                            test_only.push(dir.join(format!("{name}.rs")));
                            test_only.push(dir.join(name));
                        }
                    }
                }
                break;
            }
        }
    }
    test_only
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_status_sentence_is_written_with_a_backtick() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut offenders = Vec::new();
    let mut allowed_seen = Vec::new();
    for krate in ["dux-core", "dux-web", "dux-tui"] {
        let src = crates.join(krate).join("src");
        let mut files = Vec::new();
        rust_files(&src, &mut files);
        let test_only = test_only_files(&files);
        for file in &files {
            if test_only
                .iter()
                .any(|skip| file == skip || file.starts_with(skip))
            {
                continue;
            }
            let name = file
                .strip_prefix(crates)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let under_allowed = ALLOWED_FILES.iter().find(|(allowed, _)| {
                name == *allowed || name.starts_with(&allowed.replace(".rs", "/"))
            });
            if let Some((allowed, _)) = under_allowed {
                allowed_seen.push(*allowed);
                continue;
            }
            let source = std::fs::read_to_string(file).expect("read source");
            for line in unaccounted(&source) {
                offenders.push(format!("{name}:{line}"));
            }
        }
    }
    for (allowed, _) in ALLOWED_FILES {
        assert!(
            allowed_seen.contains(allowed),
            "{allowed} is allowlisted but no longer exists; drop it from ALLOWED_FILES"
        );
    }
    assert!(
        offenders.is_empty(),
        "these literals hold a backtick: a status shows it verbatim on both surfaces. Build \
         the sentence with status_text! and pass the command or setting as q(..) or n(..), \
         or, when the text never reaches a status, say where it goes with \
         `// backtick-ok: <reason>`:\n{}",
        offenders.join("\n")
    );
}

/// The scanner on the shapes it must tell apart.
#[test]
fn the_scan_finds_backticks_in_literals_and_nowhere_else() {
    let cases: &[(&str, &[usize])] = &[
        ("let s = \"run `x`\";\n", &[1]),
        ("let s = r#\"run `x`\"#;\n", &[1]),
        ("// a `comment`\nlet s = \"plain\";\n", &[]),
        ("/// doc `x`\nfn f() {}\n", &[]),
        ("let c = '`';\nlet s = \"plain\";\n", &[]),
        ("fn f<'a>(x: &'a str) {}\nlet s = \"`y`\";\n", &[2]),
        ("logger::warn(&format!(\"`x`\"));\n", &[]),
        (
            "let a = 1;\n// backtick-ok: printed to the console only.\nconsole.warn(\"`x`\");\n",
            &[],
        ),
        (
            "// backtick-ok: only the first.\nlet a = \"`x`\";\nlet b = \"`y`\";\n",
            &[3],
        ),
        ("let s = \"a \\\" `b`\";\n", &[1]),
        (
            "fn f() {}\n#[cfg(test)]\nmod tests {\n    const S: &str = \"`x`\";\n}\n",
            &[],
        ),
        (
            "#[cfg(test)]\nfn helper() -> &'static str { \"`x`\" }\nconst S: &str = \"`y`\";\n",
            &[3],
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(unaccounted(source), *expected, "{source}");
    }
}
