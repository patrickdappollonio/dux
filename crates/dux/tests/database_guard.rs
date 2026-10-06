//! Only the process holding `dux.lock` opens `sessions.sqlite3`. The command
//! line is a client: everything the database holds it reads and changes
//! through a running dux. This test holds that line by reading the sources the
//! command line's dispatch runs, so a branch no other test exercises is held
//! to it too.
//!
//! It reads three things:
//!
//! - the command line's own code (the dispatch in `main`, the command tree,
//!   the client commands and `dux_core::client`), which must name nothing that
//!   opens a database;
//! - every item outside that code it reaches, which must be one of [`VETTED`],
//!   each entry read and recorded as never opening the database. Reaching
//!   anything new fails until someone has read it and added it;
//! - the source of every vetted entry (a whole module, a function's body, or
//!   a type's `impl` blocks), which must name nothing that opens a database
//!   either.
//!
//! It looks one step past the client's own code; a vetted function that starts
//! calling something which opens the database is caught only where that call
//! names it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What opens a database, or reaches the code that does: the store types,
/// the SQLite crate, and the modules whose work opens the session database.
const OPENS_A_DATABASE: &[&str] = &[
    "SessionStore",
    "WebSessionStore",
    "rusqlite",
    "Connection::",
    "storage::",
    "web_sessions::",
    "process_sessions::",
    "destructive::",
];

/// What the command line's own code must not name on top of that: the
/// database's path, which nothing but opening it needs.
const NAMES_THE_DATABASE: &[&str] = &["sessions_db_path"];

#[derive(Clone, Copy)]
enum Vetted {
    /// A whole module: its file, read whole.
    Module(&'static str),
    /// A function: the file it is defined in and its name; its body is read.
    Function(&'static str, &'static str),
    /// A type: the file it is defined in and its name; its `impl` blocks are
    /// read.
    Type(&'static str, &'static str),
    /// A constant or a plain data type with no code of its own.
    Data,
    /// Opens the database on purpose, for the reason given beside it.
    Exception,
}

/// Every item outside the command line's own code that it reaches, with what
/// was read to vet it. A path is covered by an entry that is the path itself
/// or a module above it.
const VETTED: &[(&str, Vetted, &str)] = &[
    (
        "dux_tui::run_config",
        Vetted::Exception,
        "dux config: edits config.toml on this machine. Only `dux config reset --all` opens \
         the database, and only while holding dux.lock with dux stopped, to stop the programs \
         dux left running and keep the welcome-seen note",
    ),
    (
        "dux_core::reload_signal",
        Vetted::Module("crates/dux-core/src/reload_signal.rs"),
        "installs the SIGUSR1 handler and reads who holds dux.lock",
    ),
    (
        "dux_core::lockfile",
        Vetted::Module("crates/dux-core/src/lockfile.rs"),
        "reads dux.lock's text",
    ),
    (
        "dux_core::config::DuxPaths",
        Vetted::Type("crates/dux-core/src/config.rs", "DuxPaths"),
        "where dux keeps its files; naming the database's path does not open it",
    ),
    (
        "dux_core::config::ServerCliOverrides",
        Vetted::Data,
        "the listener flags of `dux server`",
    ),
    (
        "dux_core::config::load_config",
        Vetted::Function("crates/dux-core/src/config.rs", "load_config"),
        "reads config.toml",
    ),
    (
        "dux_core::config::load_config_file",
        Vetted::Function("crates/dux-core/src/config.rs", "load_config_file"),
        "reads config.toml",
    ),
    (
        "dux_core::config::check_global_env_var",
        Vetted::Function("crates/dux-core/src/config.rs", "check_global_env_var"),
        "checks a variable's name",
    ),
    (
        "dux_core::config_keys::NOT_SHOWN",
        Vetted::Data,
        "the placeholder printed for a hidden value",
    ),
    (
        "dux_core::config_resources",
        Vetted::Module("crates/dux-core/src/config_resources.rs"),
        "macros, providers, keys, themes and env as config.toml holds them",
    ),
    (
        "dux_core::attachments",
        Vetted::Module("crates/dux-core/src/attachments.rs"),
        "how an attachment and a connection are described",
    ),
    (
        "dux_core::file_follow",
        Vetted::Module("crates/dux-core/src/file_follow.rs"),
        "reads and follows the server log file",
    ),
    (
        "dux_core::logger::server_log_file",
        Vetted::Function("crates/dux-core/src/logger.rs", "server_log_file"),
        "where the server log is",
    ),
    (
        "dux_core::background_serve::TUI_DEVICE_LABEL",
        Vetted::Data,
        "the terminal UI's device label",
    ),
    (
        "dux_core::device_label",
        Vetted::Module("crates/dux-core/src/device_label.rs"),
        "shortens a browser's device label",
    ),
    (
        "dux_core::agent_tabs",
        Vetted::Module("crates/dux-core/src/agent_tabs.rs"),
        "labels an agent's tabs",
    ),
    (
        "dux_core::file_modes",
        Vetted::Module("crates/dux-core/src/file_modes.rs"),
        "owner-only files and folders",
    ),
    (
        "dux_core::io_retry",
        Vetted::Module("crates/dux-core/src/io_retry.rs"),
        "retries an interrupted system call",
    ),
    (
        "dux_core::tailscale",
        Vetted::Module("crates/dux-core/src/tailscale.rs"),
        "tells a Tailscale address apart",
    ),
    (
        "dux_core::display_version",
        Vetted::Function("crates/dux-core/src/lib.rs", "display_version"),
        "the version this binary prints",
    ),
    (
        "dux_tui::install_canonical_renderer",
        Vetted::Function("crates/dux-tui/src/config.rs", "install_canonical_renderer"),
        "registers how config.toml is written",
    ),
    (
        "dux_tui::read_sign_in_password",
        Vetted::Function("crates/dux-tui/src/config_cli.rs", "read_sign_in_password"),
        "asks for a password, or reads it from stdin",
    ),
    (
        "dux_tui::read_env_value",
        Vetted::Function("crates/dux-tui/src/config_cli.rs", "read_env_value"),
        "asks for a variable's value, or reads it from stdin",
    ),
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn read(path: &str) -> String {
    let full = workspace().join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {}: {e}", full.display()))
}

/// `text` without its unit tests and its comments: what runs.
fn code_of(text: &str) -> String {
    let mut code = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "#[cfg(test)]"
            && lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with("mod tests"))
        {
            break;
        }
        let without_comment = match line.find("//") {
            // A `//` inside a string ("http://…") is not a comment.
            Some(at) if line[..at].matches('"').count() % 2 == 0 => &line[..at],
            _ => line,
        };
        code.push_str(without_comment);
        code.push('\n');
    }
    code
}

/// The command line's own code, file by file, with the crate `crate::`
/// means in each.
fn client_sources() -> Vec<(String, &'static str, String)> {
    let mut sources = Vec::new();
    // In `main`, only `fn main` is the dispatch: the terminal UI and the
    // server it also starts are dux itself, which takes dux.lock.
    let main = code_of(&read("crates/dux/src/main.rs"));
    let dispatch = body_of(&main, "fn main()").expect("fn main in main.rs");
    sources.push(("crates/dux/src/main.rs".to_string(), "dux", dispatch));
    for file in [
        "crates/dux/src/commands.rs",
        "crates/dux/src/client_commands.rs",
    ] {
        sources.push((file.to_string(), "dux", code_of(&read(file))));
    }
    let client = workspace().join("crates/dux-core/src/client");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&client)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // A fake dux for the client's own unit tests; `mod.rs` declares it
        // under `#[cfg(test)]`.
        if name == "test_server.rs" {
            continue;
        }
        let shown = format!("crates/dux-core/src/client/{name}");
        sources.push((
            shown,
            "dux_core",
            code_of(&std::fs::read_to_string(&path).unwrap()),
        ));
    }
    sources
}

/// The text between the braces that follow the first `start` in `code`.
fn body_of(code: &str, start: &str) -> Option<String> {
    let at = code.find(start)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    let mut chars = code[open..].char_indices().peekable();
    let mut in_string = false;
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if in_string => {
                chars.next();
            }
            '"' => in_string = !in_string,
            '\'' if !in_string => {
                // A brace as a character literal: '{' or '}'.
                let rest = &code[open + i..];
                if rest.starts_with("'{'") || rest.starts_with("'}'") {
                    chars.next();
                    chars.next();
                }
            }
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(code[open..=open + i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Every `fn name` definition's body in `code` (a name defined in more than
/// one `impl` is read in each).
fn function_bodies(code: &str, name: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    let needle = format!("fn {name}");
    let mut from = 0;
    while let Some(found) = code[from..].find(&needle) {
        let at = from + found;
        let after = code[at + needle.len()..].chars().next();
        if matches!(after, Some('(' | '<')) {
            bodies.extend(body_of(&code[at..], &needle));
        }
        from = at + needle.len();
    }
    bodies
}

/// The bodies of every `impl` block for `ty` in `code`, trait impls included.
fn impl_bodies(code: &str, ty: &str) -> Vec<String> {
    let mut bodies = Vec::new();
    for (at, _) in code.match_indices("impl") {
        let header_end = match code[at..].find('{') {
            Some(end) => at + end,
            None => continue,
        };
        let header = &code[at..header_end];
        let names_type = header
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .any(|word| word == ty);
        let starts_item = code[..at]
            .chars()
            .last()
            .is_none_or(|c| c == '\n' || c == ' ');
        if names_type && starts_item && !header.contains(';') {
            bodies.extend(body_of(&code[at..], "impl"));
        }
    }
    bodies
}

/// Every `use` tree in `code`, expanded to the paths it imports, and the
/// code with the `use` statements taken out.
fn split_uses(code: &str) -> (Vec<String>, String) {
    let mut paths = Vec::new();
    let mut rest = String::new();
    let mut remaining = code;
    while let Some(at) = find_use(remaining) {
        rest.push_str(&remaining[..at]);
        let after = &remaining[at..];
        let end = after.find(';').expect("a use statement ends");
        let tree = tidy(&after["use".len()..end]);
        expand(&tree, "", &mut paths);
        remaining = &after[end + 1..];
    }
    rest.push_str(remaining);
    (paths, rest)
}

/// Where the next `use` statement starts: `use` (or `pub use`) as a whole
/// word followed by whitespace.
fn find_use(code: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(found) = code[from..].find("use ") {
        let at = from + found;
        let before = code[..at].chars().last();
        if before.is_none_or(|c| c == '\n' || c == ' ' || c == '{' || c == ';') {
            return Some(at);
        }
        from = at + 4;
    }
    None
}

/// A `use` tree on one line, with no space but the one in ` as `.
fn tidy(tree: &str) -> String {
    let mut one_line = tree.split_whitespace().collect::<Vec<_>>().join(" ");
    for mark in ["{", "}", ",", "::"] {
        one_line = one_line
            .replace(&format!(" {mark}"), mark)
            .replace(&format!("{mark} "), mark);
    }
    one_line
}

fn expand(tree: &str, prefix: &str, out: &mut Vec<String>) {
    let join = |segment: &str| {
        if prefix.is_empty() {
            segment.to_string()
        } else {
            format!("{prefix}::{segment}")
        }
    };
    let Some(open) = tree.find('{') else {
        let path = tree.split(" as ").next().unwrap_or(tree);
        match path {
            "self" => out.push(prefix.to_string()),
            "*" => out.push(join("*")),
            _ => out.push(join(path)),
        }
        return;
    };
    let head = tree[..open].trim_end_matches("::");
    let prefix = if head.is_empty() {
        prefix.to_string()
    } else {
        join(head)
    };
    let inner = &tree[open + 1..tree.len() - 1];
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                if !inner[start..i].is_empty() {
                    expand(&inner[start..i], &prefix, out);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if !inner[start..].is_empty() {
        expand(&inner[start..], &prefix, out);
    }
}

/// Every path rooted at `crate`, `dux_core` or `dux_tui` that `code` names
/// outside its `use` statements.
fn inline_paths(code: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for root in ["crate::", "dux_core::", "dux_tui::"] {
        for (at, _) in code.match_indices(root) {
            let before = code[..at].chars().last();
            if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':') {
                continue;
            }
            let path: String = code[at..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':')
                .collect();
            paths.push(path.trim_end_matches(':').to_string());
        }
    }
    paths
}

/// `path` with `crate` replaced by the crate it means, or `None` for a path
/// inside the command line's own code.
fn outside(path: &str, krate: &str) -> Option<String> {
    let path = match path.strip_prefix("crate::") {
        Some(rest) => format!("{krate}::{rest}"),
        None => path.to_string(),
    };
    let own = path == "dux_core::client"
        || path.starts_with("dux_core::client::")
        || path.starts_with("dux::")
        || !(path.starts_with("dux_core::") || path.starts_with("dux_tui::"));
    (!own).then_some(path)
}

fn vetted_for(path: &str) -> Option<&'static (&'static str, Vetted, &'static str)> {
    VETTED
        .iter()
        .find(|(entry, _, _)| path == *entry || path.starts_with(&format!("{entry}::")))
}

fn named(code: &str, tokens: &[&str]) -> Vec<String> {
    tokens
        .iter()
        .filter(|token| code.contains(**token))
        .map(|token| token.to_string())
        .collect()
}

#[test]
fn the_command_line_never_opens_the_session_database() {
    let mut problems = Vec::new();
    let mut reached = BTreeSet::new();

    for (file, krate, code) in client_sources() {
        let forbidden: Vec<&str> = OPENS_A_DATABASE
            .iter()
            .chain(NAMES_THE_DATABASE)
            .copied()
            .collect();
        for token in named(&code, &forbidden) {
            problems.push(format!("{file} names {token}"));
        }
        if code.contains("super::super") {
            problems.push(format!(
                "{file} reaches out of its module through super::super; name the item by its \
                 crate path so it can be vetted"
            ));
        }
        let (uses, rest) = split_uses(&code);
        for path in uses.iter().chain(inline_paths(&rest).iter()) {
            if let Some(path) = outside(path, krate) {
                match vetted_for(&path) {
                    Some((entry, _, _)) => {
                        reached.insert(*entry);
                    }
                    None => problems.push(format!(
                        "{file} reaches {path}, which nobody has vetted: read it and everything \
                         it calls, and if none of it opens sessions.sqlite3 add it to VETTED \
                         with what it does"
                    )),
                }
            }
        }
    }

    for (entry, vetted, _) in VETTED {
        if !reached.contains(entry) {
            problems.push(format!(
                "{entry} is vetted but the command line no longer reaches it; take it out of \
                 VETTED"
            ));
        }
        let read_code = |file: &str| code_of(&read(file));
        let bodies = match *vetted {
            Vetted::Module(file) => vec![read_code(file)],
            Vetted::Function(file, name) => function_bodies(&read_code(file), name),
            Vetted::Type(file, name) => impl_bodies(&read_code(file), name),
            Vetted::Data | Vetted::Exception => continue,
        };
        if bodies.is_empty() {
            problems.push(format!(
                "{entry}'s source was not found where VETTED says it is"
            ));
        }
        for body in bodies {
            for token in named(&body, OPENS_A_DATABASE) {
                problems.push(format!(
                    "{entry}, which the command line reaches, names {token}"
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "only the process holding dux.lock opens sessions.sqlite3; the command line reaches it \
         only through a running dux:\n  {}",
        problems.join("\n  ")
    );
}
