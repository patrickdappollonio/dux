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
