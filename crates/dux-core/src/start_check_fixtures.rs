//! Config files that stop, or do not stop, each surface's start, shared by
//! the tests that check every surface against the one list of start checks
//! (`crate::config::check_start`). A surface's start path refusing a file
//! the list calls clean for it, or accepting one the list says it refuses,
//! fails those tests.

/// One file and which surfaces will not start with it.
pub struct Fixture {
    /// What the file is, for a failure message.
    pub name: &'static str,
    /// The whole config file.
    pub text: &'static str,
    /// Whether the terminal UI refuses it (at start and on reload).
    pub stops_terminal_ui: bool,
    /// Whether `dux server` refuses it, with no `--bind` or `--port`.
    pub stops_dux_server: bool,
}

/// Every kind of problem the start checks know, each in its own file, and
/// a clean file.
pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "a clean file",
        text: "[ui]\nleft_width_pct = 25\n",
        stops_terminal_ui: false,
        stops_dux_server: false,
    },
    Fixture {
        name: "not TOML",
        text: "[ui\nleft_width_pct = 25\n",
        stops_terminal_ui: true,
        stops_dux_server: true,
    },
    Fixture {
        name: "a password hash directly under [server]",
        text: "[server]\npassword_hash = \"x\"\n",
        stops_terminal_ui: true,
        stops_dux_server: true,
    },
    Fixture {
        name: "an environment variable and a provider named password_hash",
        text: "[env]\npassword_hash = \"x\"\n\n[providers.password_hash]\ncommand = \"mytool\"\n",
        stops_terminal_ui: false,
        stops_dux_server: false,
    },
    Fixture {
        name: "a [server.auth] value out of range",
        text: "[server.auth]\nsession_idle_seconds = 0\n",
        stops_terminal_ui: true,
        stops_dux_server: true,
    },
    Fixture {
        name: "a wrong-typed setting",
        text: "[ui]\nleft_width_pct = \"wide\"\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
    Fixture {
        name: "a section that is not a table",
        text: "ui = 5\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
    Fixture {
        name: "a host that is not an IP",
        text: "[server]\nhost = \"localhost\"\n",
        stops_terminal_ui: true,
        stops_dux_server: true,
    },
    Fixture {
        name: "port 0, not serving beside the terminal UI",
        text: "[server]\nport = 0\n",
        stops_terminal_ui: false,
        stops_dux_server: true,
    },
    Fixture {
        name: "port 0, serving beside the terminal UI",
        text: "[server]\nport = 0\nserve_while_tui = true\n",
        stops_terminal_ui: false,
        stops_dux_server: true,
    },
    Fixture {
        name: "an environment value with broken expansion",
        text: "[env]\nA = \"${\"\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
    Fixture {
        name: "duplicate project ids",
        text: "[[projects]]\nid = \"same\"\npath = \"/tmp/dux-fixture-a\"\n\n[[projects]]\nid = \"same\"\npath = \"/tmp/dux-fixture-b\"\n",
        stops_terminal_ui: true,
        stops_dux_server: true,
    },
    Fixture {
        name: "a deprecated key of the wrong type the migrations cannot carry over",
        text: "[defaults]\nprompt_for_name = \"yes\"\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
    Fixture {
        name: "a deprecated [server] bind that is not a string",
        text: "[server]\nbind = 5\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
    Fixture {
        name: "[keys] the terminal UI does not accept",
        text: "[keys]\nnot_a_real_action = [\"x\"]\n",
        stops_terminal_ui: true,
        stops_dux_server: false,
    },
];

/// One case of the start corpus (`tests/fixtures/start_corpus.txt`): a
/// whole config file and whether each surface starts with it, as pinned to
/// the release before the config checks were rebuilt.
pub struct CorpusCase {
    /// The whole config file.
    pub text: String,
    /// Whether the terminal UI starts with it.
    pub terminal_ui_starts: bool,
    /// Whether `dux server` starts with it, with no `--bind` or `--port`.
    pub dux_server_starts: bool,
}

/// Every case of the start corpus, read from its fixture (see the comment
/// at its head for the format and for each outcome changed on purpose).
pub fn start_corpus() -> Vec<CorpusCase> {
    const TEXT: &str = include_str!("../tests/fixtures/start_corpus.txt");
    let starts = |outcome: &str| match outcome.split('(').next() {
        Some("start") => true,
        Some("refuse") => false,
        _ => panic!("start corpus: unknown outcome {outcome:?}"),
    };
    TEXT.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (Some(tui), Some(server), Some(body)) = (parts.next(), parts.next(), parts.next())
            else {
                panic!("start corpus: malformed line {line:?}");
            };
            CorpusCase {
                text: serde_json::from_str(body)
                    .unwrap_or_else(|e| panic!("start corpus: {line:?}: {e}")),
                terminal_ui_starts: starts(tui),
                dux_server_starts: starts(server),
            }
        })
        .collect()
}

/// Every structural position a name the file writes can take, `{T}`
/// standing for it: each fixed table, each map of user-chosen names, under
/// values that are not tables, and under misplaced password hashes. For the
/// tests that a name that is not a setting name never reaches a printer.
pub const NAME_POSITIONS: &[&str] = &[
    "\"{T}\" = 1\n",
    "[\"{T}\"]\nx = 1\n",
    "[\"{T}\"]\npassword_hash = \"x\"\n",
    "[ui]\n\"{T}\" = 5\n",
    "[ui]\n\"{T}\" = { a = 1 }\n",
    "[ui]\nleft_width_pct = { \"{T}\" = 1 }\n",
    "[defaults]\n\"{T}\" = 1\n",
    "[defaults]\n\"{T}\" = { password_hash = \"x\" }\n",
    "[logging]\n\"{T}\" = 1\n",
    "[capabilities]\n\"{T}\" = 1\n",
    "[editor]\n\"{T}\" = 1\n",
    "[terminal]\n\"{T}\" = 1\n",
    "[startup_command_terminal]\n\"{T}\" = 1\n",
    "[server]\n\"{T}\" = 1\n",
    "[server]\n\"{T}\" = { password_hash = \"x\" }\n",
    "[server]\nbind = { \"{T}\" = 1 }\n",
    "[server.\"{T}\"]\nrequire = \"network\"\n",
    "[server.auth]\n\"{T}\" = 5\n",
    "[env]\n\"{T}\" = \"x\"\n",
    "[env]\n\"{T}\" = 5\n",
    "[env]\nA = { \"{T}\" = 1 }\n",
    "[providers.\"{T}\"]\ncommand = \"x\"\n",
    "[providers.\"{T}\"]\nargs = 5\n",
    "[providers.claude]\n\"{T}\" = 5\n",
    "[providers.mytool]\ncommand = \"x\"\n\"{T}\" = 5\n",
    "[providers.mytool]\ncommand = \"x\"\nargs = { \"{T}\" = 1 }\n",
    "[macros]\n\"{T}\" = \"x\"\n",
    "[macros.\"{T}\"]\ntext = 1\n",
    "[macros.m]\ntext = \"x\"\n\"{T}\" = 1\n",
    "[keys]\n\"{T}\" = [\"ctrl-q\"]\n",
    "[keys]\n\"{T}\" = 5\n",
    "[keys]\nquit = { \"{T}\" = 1 }\n",
    "[[projects]]\nid = \"p\"\npath = \"/tmp/dux-names-p\"\n\"{T}\" = 1\n",
    "[[projects]]\nid = \"p\"\npath = \"/tmp/dux-names-p\"\n[projects.env]\n\"{T}\" = \"x\"\n",
    // Inside arrays: an array of tables where the schema has a table or a
    // map, an unknown array, a table inside a list setting (nested too), and
    // the keys of a project entry written inline or a level down.
    "[[ui]]\n\"{T}\" = 1\n",
    "[[providers]]\n\"{T}\" = 1\n",
    "[[macros]]\n\"{T}\" = 1\n",
    "[[extra]]\n\"{T}\" = 1\n",
    "[server]\nauth = [{ \"{T}\" = 1 }]\n",
    "[server]\nallowed_hosts = [{ \"{T}\" = 1 }]\n",
    "[server]\nallowed_hosts = [[{ \"{T}\" = 1 }]]\n",
    "[env]\nA = [{ \"{T}\" = 1 }]\n",
    "[providers.mytool]\ncommand = \"x\"\nargs = [{ \"{T}\" = 1 }]\n",
    "[keys]\nquit = [{ \"{T}\" = 1 }]\n",
    "projects = [{ id = \"p\", path = \"/tmp/dux-names-p\", \"{T}\" = 1 }]\n",
    "[[projects]]\nid = \"p\"\npath = \"/tmp/dux-names-p\"\n[[projects.extra]]\n\"{T}\" = 1\n",
    // Nested under a key the schema has no place for, in every section: the
    // unknown key hides, and so does everything below it.
    "[ui]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[ui]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[ui.zz_unknown]\n\"{T}\" = 1\n",
    "[defaults]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[defaults]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[defaults.zz_unknown]\n\"{T}\" = 1\n",
    "[logging]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[logging]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[logging.zz_unknown]\n\"{T}\" = 1\n",
    "[capabilities]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[capabilities]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[capabilities.zz_unknown]\n\"{T}\" = 1\n",
    "[editor]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[editor]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[editor.zz_unknown]\n\"{T}\" = 1\n",
    "[terminal]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[terminal]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[terminal.zz_unknown]\n\"{T}\" = 1\n",
    "[startup_command_terminal]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[startup_command_terminal]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[startup_command_terminal.zz_unknown]\n\"{T}\" = 1\n",
    "[server]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[server]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[server.zz_unknown]\n\"{T}\" = 1\n",
    "[server.auth]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[server.auth]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[server.auth.zz_unknown]\n\"{T}\" = 1\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = { \"{T}\" = 1 }\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[providers.mytool.zz_unknown]\n\"{T}\" = 1\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = { \"{T}\" = 1 }\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[macros.m.zz_unknown]\n\"{T}\" = 1\n",
    "[keys]\nzz_unknown = { \"{T}\" = 1 }\n",
    "[keys]\nzz_unknown = [{ \"{T}\" = 1 }]\n",
    "[keys.zz_unknown]\n\"{T}\" = 1\n",
    "zz_unknown = { \"{T}\" = 1 }\n",
    "[[projects]]\nid = \"p\"\npath = \"/tmp/dux-names-p\"\nzz_unknown = { \"{T}\" = { a = 1 } }\n",
];

/// Where a VALUE sits below a key the formatter does not name, `{V}` standing
/// for it: an unknown key and a name that breaks its map's rule in every
/// section, the value plain, in a list (nested too) and in a table; a whole
/// entry whose name breaks its map's rule; and a setting written as a table
/// (`set` reports what it replaced). No printer may repeat such a value.
/// (Under `[keys]` an unknown name that follows the action-name rule is a
/// name the formatter does print, and its value a binding, so only the
/// table forms and a name breaking the rule are hidden there.)
pub const VALUE_POSITIONS: &[&str] = &[
    "[ui]\nzz_unknown = \"{V}\"\n",
    "[ui]\nzz_unknown = [\"{V}\"]\n",
    "[ui]\nzz_unknown = [[\"{V}\"]]\n",
    "[ui]\nzz_unknown = { a = \"{V}\" }\n",
    "[ui]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[ui]\n\"bad name\" = \"{V}\"\n",
    "[ui.zz_unknown]\na = \"{V}\"\n",
    "[defaults]\nzz_unknown = \"{V}\"\n",
    "[defaults]\nzz_unknown = [\"{V}\"]\n",
    "[defaults]\nzz_unknown = [[\"{V}\"]]\n",
    "[defaults]\nzz_unknown = { a = \"{V}\" }\n",
    "[defaults]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[defaults]\n\"bad name\" = \"{V}\"\n",
    "[defaults.zz_unknown]\na = \"{V}\"\n",
    "[logging]\nzz_unknown = \"{V}\"\n",
    "[logging]\nzz_unknown = [\"{V}\"]\n",
    "[logging]\nzz_unknown = [[\"{V}\"]]\n",
    "[logging]\nzz_unknown = { a = \"{V}\" }\n",
    "[logging]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[logging]\n\"bad name\" = \"{V}\"\n",
    "[logging.zz_unknown]\na = \"{V}\"\n",
    "[capabilities]\nzz_unknown = \"{V}\"\n",
    "[capabilities]\nzz_unknown = [\"{V}\"]\n",
    "[capabilities]\nzz_unknown = [[\"{V}\"]]\n",
    "[capabilities]\nzz_unknown = { a = \"{V}\" }\n",
    "[capabilities]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[capabilities]\n\"bad name\" = \"{V}\"\n",
    "[capabilities.zz_unknown]\na = \"{V}\"\n",
    "[editor]\nzz_unknown = \"{V}\"\n",
    "[editor]\nzz_unknown = [\"{V}\"]\n",
    "[editor]\nzz_unknown = [[\"{V}\"]]\n",
    "[editor]\nzz_unknown = { a = \"{V}\" }\n",
    "[editor]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[editor]\n\"bad name\" = \"{V}\"\n",
    "[editor.zz_unknown]\na = \"{V}\"\n",
    "[terminal]\nzz_unknown = \"{V}\"\n",
    "[terminal]\nzz_unknown = [\"{V}\"]\n",
    "[terminal]\nzz_unknown = [[\"{V}\"]]\n",
    "[terminal]\nzz_unknown = { a = \"{V}\" }\n",
    "[terminal]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[terminal]\n\"bad name\" = \"{V}\"\n",
    "[terminal.zz_unknown]\na = \"{V}\"\n",
    "[startup_command_terminal]\nzz_unknown = \"{V}\"\n",
    "[startup_command_terminal]\nzz_unknown = [\"{V}\"]\n",
    "[startup_command_terminal]\nzz_unknown = [[\"{V}\"]]\n",
    "[startup_command_terminal]\nzz_unknown = { a = \"{V}\" }\n",
    "[startup_command_terminal]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[startup_command_terminal]\n\"bad name\" = \"{V}\"\n",
    "[startup_command_terminal.zz_unknown]\na = \"{V}\"\n",
    "[server]\nzz_unknown = \"{V}\"\n",
    "[server]\nzz_unknown = [\"{V}\"]\n",
    "[server]\nzz_unknown = [[\"{V}\"]]\n",
    "[server]\nzz_unknown = { a = \"{V}\" }\n",
    "[server]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[server]\n\"bad name\" = \"{V}\"\n",
    "[server.zz_unknown]\na = \"{V}\"\n",
    "[server.auth]\nzz_unknown = \"{V}\"\n",
    "[server.auth]\nzz_unknown = [\"{V}\"]\n",
    "[server.auth]\nzz_unknown = [[\"{V}\"]]\n",
    "[server.auth]\nzz_unknown = { a = \"{V}\" }\n",
    "[server.auth]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[server.auth]\n\"bad name\" = \"{V}\"\n",
    "[server.auth.zz_unknown]\na = \"{V}\"\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = \"{V}\"\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = [\"{V}\"]\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = [[\"{V}\"]]\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = { a = \"{V}\" }\n",
    "[providers.mytool]\ncommand = \"x\"\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[providers.mytool]\ncommand = \"x\"\n\"bad name\" = \"{V}\"\n",
    "[providers.mytool.zz_unknown]\na = \"{V}\"\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = \"{V}\"\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = [\"{V}\"]\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = [[\"{V}\"]]\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = { a = \"{V}\" }\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[macros.m]\ntext = \"x\"\nsurface = \"agent\"\n\"bad name\" = \"{V}\"\n",
    "[macros.m.zz_unknown]\na = \"{V}\"\n",
    "[keys]\nzz_unknown = { a = \"{V}\" }\n",
    "[keys]\nzz_unknown = [{ a = \"{V}\" }]\n",
    "[keys]\n\"bad name\" = \"{V}\"\n",
    "[keys.zz_unknown]\na = \"{V}\"\n",
    "zz_unknown = \"{V}\"\n",
    "\"bad name\" = [\"{V}\"]\n",
    "[providers.\"bad name\"]\ncommand = \"{V}\"\n",
    "[providers.\"bad name\"]\ncommand = 5\nargs = [\"{V}\"]\n",
    "[macros.\"bad name\"]\ntext = \"{V}\"\nsurface = \"agent\"\n",
    "[keys]\n\"Bad Name\" = [\"{V}\"]\n",
    "[[projects]]\nid = \"p\"\npath = \"/tmp/dux-names-p\"\nzz_unknown = \"{V}\"\n",
    "[[extra]]\na = \"{V}\"\n",
    "[ui]\ntheme = [[{ a = \"{V}\" }]]\n",
    "[ui.theme]\na = \"{V}\"\n",
];

/// Where a `[keys]` binding's VALUE sits, `{V}` standing for it, in every way
/// `[keys]` can be written. A binding is a value dux prints in `get keys`, as
/// it prints any setting's value, and nowhere else: no problem, refusal or
/// `set` message may repeat it.
pub const BINDING_VALUE_POSITIONS: &[&str] = &[
    "[keys]\nquit = [\"{V}\"]\n",
    "[keys]\nquit = [\"ctrl-q\", \"{V}\"]\n",
    "[keys]\nopen_palette = [\"{V}\"]\nquit = [\"{V}\"]\n",
    "keys = { quit = [\"{V}\"] }\n",
    "keys.quit = [\"{V}\"]\n",
];

/// Where a project's PATH, NAME or ID sits, `{V}` standing for it, beside a
/// problem that names the project (a broken env value, a duplicate record).
/// Projects are sensitive in every printer: a problem names a project by its
/// line, never by its path, name or id.
pub const PROJECT_VALUE_POSITIONS: &[&str] = &[
    "[[projects]]\nid = \"a\"\npath = \"/tmp/{V}\"\nenv = { FOO = \"${\" }\n",
    "[[projects]]\nid = \"a\"\nname = \"{V}\"\npath = \"/tmp/a\"\nenv = { FOO = \"${\" }\n",
    "[[projects]]\nid = \"{V}\"\npath = \"/tmp/a\"\nenv = { FOO = \"${\" }\n",
    "[[projects]]\nid = \"a\"\npath = \"/tmp/{V}\"\n[[projects]]\nid = \"b\"\npath = \"/tmp/{V}\"\n",
    "[[projects]]\nid = \"{V}\"\npath = \"/tmp/a\"\n[[projects]]\nid = \"{V}\"\npath = \"/tmp/b\"\n",
    "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\nname = \"{V}\"\n[projects.env]\n\"1BAD\" = \"x\"\n",
];

/// Token-like names, from a fixed seed: each holds a dot and a space, so it
/// breaks every naming rule, and starts with a marker no setting has.
pub fn name_tokens() -> Vec<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    // A fixed seed, so a failure is the same failure every run.
    let mut state: u64 = 0x005e_ed0f_d0c5;
    (0..4)
        .map(|_| {
            let body: String = (0..20)
                .map(|_| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ALPHABET[(state >> 33) as usize % ALPHABET.len()] as char
                })
                .collect();
            format!("zzTOKEN{}.{} x", &body[..10], &body[10..])
        })
        .collect()
}
