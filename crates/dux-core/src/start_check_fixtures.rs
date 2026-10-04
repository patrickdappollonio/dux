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
