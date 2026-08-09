//! dux configuration data model: the serde structs persisted in `config.toml`
//! and their defaults, plus `DuxPaths` and path resolution (`resolve_root`,
//! `discover_root`) and the env-expansion helpers (`expand_env_vars`,
//! `expand_path`, `resolve_project_env`, …). The keybinding-aware *renderer* of
//! the documented config, plus `ensure_config`/`save_config` orchestration and
//! the toml_edit patching, live in the binary's `config` module — not here.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which surface(s) a macro is available on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MacroSurface {
    #[default]
    Agent,
    Terminal,
    Both,
}

impl MacroSurface {
    /// Human-readable label for UI display.
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent only",
            Self::Terminal => "terminal only",
            Self::Both => "agent + terminal",
        }
    }

    /// The canonical config/wire string for this surface, matching the
    /// `#[serde(rename_all = "lowercase")]` representation. Use this anywhere a
    /// `MacroSurface` crosses into TOML or JSON so the casing stays in one place.
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
            Self::Both => "both",
        }
    }

    /// Parse the canonical config/wire string back into a `MacroSurface`.
    /// Returns `None` for an unrecognized value.
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(Self::Agent),
            "terminal" => Some(Self::Terminal),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// Cycle to the next variant: Agent -> Terminal -> Both -> Agent.
    pub fn next(self) -> Self {
        match self {
            Self::Agent => Self::Terminal,
            Self::Terminal => Self::Both,
            Self::Both => Self::Agent,
        }
    }

    /// Cycle to the previous variant: Agent -> Both -> Terminal -> Agent.
    pub fn prev(self) -> Self {
        match self {
            Self::Agent => Self::Both,
            Self::Both => Self::Terminal,
            Self::Terminal => Self::Agent,
        }
    }

    /// Whether this surface matches the given session surface.
    pub fn matches(self, session: crate::model::SessionSurface) -> bool {
        match self {
            Self::Both => true,
            Self::Agent => session == crate::model::SessionSurface::Agent,
            Self::Terminal => session == crate::model::SessionSurface::Terminal,
        }
    }
}

/// A single text macro entry with surface restriction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroEntry {
    pub text: String,
    pub surface: MacroSurface,
}

/// Text macros: a map from name to entry.
/// Each entry is triggered from the macro bar (Ctrl+\).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MacrosConfig {
    #[serde(flatten)]
    pub entries: IndexMap<String, MacroEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub provider: String,
    pub start_directory: Option<String>,
    pub enable_randomized_pet_name_by_default: bool,
    #[serde(default = "default_true")]
    pub pull_before_creating_agent_by_default: bool,
    #[serde(default = "default_true")]
    pub copy_uncommitted_changes_by_default: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    #[serde(flatten)]
    pub commands: IndexMap<String, ProviderCommandConfig>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupCommandTerminalConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub default: String,
}

/// Default cap on concurrent events (`/ws`) WebSocket connections; see
/// [`ServerConfig::max_websocket_events_connections`]. Shared so the config
/// default and the server's router default cannot drift apart.
pub const DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS: u32 = 32;
/// Default cap on concurrent agent-PTY WebSocket connections — see
/// [`ServerConfig::max_websocket_agent_connections`].
pub const DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS: u32 = 32;
/// Default cap on concurrent terminal-PTY WebSocket connections — see
/// [`ServerConfig::max_websocket_terminal_connections`].
pub const DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS: u32 = 64;
/// Default cap on concurrent extra-tab PTY WebSocket connections across ALL
/// agents — see [`ServerConfig::max_websocket_tab_connections`]. A pool of its
/// own so tab sockets can never starve the agent-PTY (session-slot tab) pool.
pub const DEFAULT_MAX_WEBSOCKET_TAB_CONNECTIONS: u32 = 64;

/// Default per-agent cap on concurrent live extra-tab PTY sockets, checked
/// before a permit is taken from the shared tab pool — see
/// [`ServerConfig::max_websocket_tabs_per_agent`]. Keeps one agent's tabs from
/// monopolizing that pool and starving other agents' tabs.
pub const DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT: u32 = 8;

/// Default per-agent tab cap (see [`UiConfig::agent_tabs_max`]), counting the
/// session-slot tab — so the default 20 allows the session-slot tab plus 19 extra tabs.
pub const DEFAULT_AGENT_TABS_MAX: u16 = 20;
/// Hard ceiling the per-agent tab cap is clamped to, so a fat-fingered config
/// value can't ask the app to keep unbounded live PTYs per agent.
pub const MAX_AGENT_TABS_MAX: u16 = 100;

/// The effective per-agent tab cap: `0` (or an absent key) means "use the
/// default"; larger values are clamped to [`MAX_AGENT_TABS_MAX`] with a warning,
/// mirroring [`shutdown_grace`]'s clamp-at-use discipline so a bad value degrades
/// gracefully instead of nuking the setting.
pub fn normalized_agent_tabs_max(configured: u16) -> u16 {
    if configured == 0 {
        return DEFAULT_AGENT_TABS_MAX;
    }
    if configured > MAX_AGENT_TABS_MAX {
        crate::logger::warn(&format!(
            "[ui] agent_tabs_max = {configured} exceeds the maximum of \
             {MAX_AGENT_TABS_MAX} and is being clamped",
        ));
        return MAX_AGENT_TABS_MAX;
    }
    configured
}

/// Default cap on the file-search index flat walk (see
/// [`crate::git::worktree_files`]). The web editor's file TREE is a lazy,
/// per-directory browser and is never capped; this only bounds the flat list
/// that backs the editor's "Search files…" box, where an incomplete result on
/// a giant repo (e.g. a built `target/`) is acceptable. `0` disables the cap.
pub const DEFAULT_SEARCH_INDEX_MAX_FILES: usize = 50_000;

/// Default cap on concurrent `/files/tree` directory listings (see
/// [`crate::git::list_dir`]). Each listing does one blocking `read_dir` off
/// the async reactor; this bounds how many can run at once so a burst of tree
/// requests can't exhaust the server's blocking-thread pool. `0` disables the
/// bound entirely (unlimited concurrency), unlike the `0 = block everything`
/// convention used by the `max_websocket_*_connections` family.
pub const DEFAULT_TREE_LIST_MAX_CONCURRENCY: u32 = 8;

/// Default cap on concurrent release-notes fetches (see
/// [`crate::release_notes::load_release_notes_from`]). Each one is a blocking
/// HTTPS round trip to the GitHub API run off the async reactor, and every
/// browser tab can ask for it from the app menu. Small on purpose: the answer
/// is cached with a six-hour TTL and identical for every caller, so a handful
/// of in-flight fetches is already more than the work needs. `0` disables the
/// bound entirely (unlimited concurrency), matching
/// [`DEFAULT_TREE_LIST_MAX_CONCURRENCY`] rather than the `0 = block
/// everything` convention of the `max_websocket_*_connections` family.
pub const DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY: u32 = 2;

/// Default per-file size cap for a file dropped onto a web terminal or agent
/// pane, in bytes (100 MiB).
///
/// It is set EXPLICITLY on the upload route because the web framework's own
/// default body limit is 2 MB, which would reject an ordinary screenshot on a
/// high-resolution display. `0` disables file drop entirely, matching how other
/// zero-valued settings in dux read as off.
pub const DEFAULT_FILE_DROP_MAX_BYTES: usize = 100 * 1024 * 1024;

/// Default cap on how many dropped-file uploads may be in flight at once.
///
/// The permit is taken BEFORE the request body is read, so together with
/// [`DEFAULT_FILE_DROP_MAX_BYTES`] this bounds the worst case at roughly two
/// hundred megabytes of buffered upload. Unlike the `max_websocket_*` family,
/// `0` here does not block everything: it clamps to 1, because a zero-permit
/// semaphore would deadlock every drop forever rather than disabling a feature.
/// Disabling file drop is what the size cap's `0` is for.
pub const DEFAULT_FILE_DROP_MAX_CONCURRENCY: u32 = 2;

/// Default seconds to wait for SIGTERMed agents/terminals to exit before
/// force-killing them on shutdown. Shared by the top-level
/// [`Config::shutdown_timeout_seconds`] (TUI quit) and
/// [`ServerConfig::shutdown_timeout_seconds`] (web/`dux server`).
pub const DEFAULT_SHUTDOWN_TIMEOUT_SECONDS: u16 = 30;

/// Hard ceiling on a configured shutdown timeout. The field is seconds, but a
/// `u16` reaches ~18 hours, so a fat-fingered millisecond value (e.g. `30000`)
/// would otherwise block quit — and, on the web server, the single engine
/// thread that services every client — for that whole time. Clamping keeps a
/// misconfiguration from wedging shutdown while still allowing any sane grace.
pub const MAX_SHUTDOWN_TIMEOUT_SECONDS: u16 = 600;

/// Default seconds between blind GitHub PR-status safety polls. Deliberately
/// slow: most PR updates arrive via events (a branch push, or focusing an
/// agent), so this backstop only needs to catch changes made on GitHub itself.
pub const DEFAULT_PR_POLL_INTERVAL_SECONDS: u16 = 180;

/// Hard ceiling on the PR poll interval (6 hours). Already far slower than any
/// useful safety net, so cap there to keep a fat-fingered value from silently
/// neutering the backstop while still allowing any sane interval.
pub const MAX_PR_POLL_INTERVAL_SECONDS: u16 = 21_600;

/// Floor on a *nonzero* PR poll interval. `0` disables the blind poll entirely;
/// any positive value below this is clamped up so a mistyped tiny value (e.g.
/// `1`) can't hammer the GitHub API every second.
pub const MIN_PR_POLL_INTERVAL_SECONDS: u16 = 30;

/// Normalize a configured `pr_poll_interval_seconds`: `0` is a valid
/// "disable the blind poll" value and is preserved; any other value is clamped
/// into `[MIN_PR_POLL_INTERVAL_SECONDS, MAX_PR_POLL_INTERVAL_SECONDS]` (with a
/// warning) so a fat-fingered entry can't hammer the API or neuter the backstop.
pub fn normalized_pr_poll_interval(seconds: u16) -> u16 {
    if seconds == 0 {
        return 0;
    }
    if seconds > MAX_PR_POLL_INTERVAL_SECONDS {
        crate::logger::warn(&format!(
            "pr_poll_interval_seconds = {seconds} exceeds the maximum of \
             {MAX_PR_POLL_INTERVAL_SECONDS}s and is being clamped."
        ));
        return MAX_PR_POLL_INTERVAL_SECONDS;
    }
    if seconds < MIN_PR_POLL_INTERVAL_SECONDS {
        crate::logger::warn(&format!(
            "pr_poll_interval_seconds = {seconds} is below the minimum of \
             {MIN_PR_POLL_INTERVAL_SECONDS}s and is being clamped (use 0 to disable the poll)."
        ));
        return MIN_PR_POLL_INTERVAL_SECONDS;
    }
    seconds
}

/// Hard ceiling on `ui.status_clear_seconds` (the settings-PATCH endpoint
/// clamps to this). One hour is far longer than any useful auto-clear delay;
/// `0` ("never auto-clear") is a separate, always-allowed value handled by the
/// caller before clamping.
pub const MAX_STATUS_CLEAR_SECONDS: u16 = 3_600;

/// Default `ui.terminal_font_size`, in pixels. Matches xterm's own default and
/// the web `terminalFont.ts` helper's fallback.
pub const DEFAULT_TERMINAL_FONT_SIZE: u16 = 14;

/// Floor on `ui.terminal_font_size`. Below this the glyphs are unreadable.
pub const MIN_TERMINAL_FONT_SIZE: u16 = 8;

/// Ceiling on `ui.terminal_font_size`. Above this a terminal pane can no
/// longer usefully fit any real width.
pub const MAX_TERMINAL_FONT_SIZE: u16 = 32;

/// Normalize a configured `terminal_font_size`: a value outside
/// [`MIN_TERMINAL_FONT_SIZE`, `MAX_TERMINAL_FONT_SIZE`] degrades to
/// [`DEFAULT_TERMINAL_FONT_SIZE`] rather than being clamped to the nearer
/// bound, so a config value that is merely wrong (not almost right) reads as
/// an obviously-reset default instead of a silently-nudged number.
///
/// Deliberately PURE: it does not log. The on-disk value is warned about and
/// corrected exactly once, at load, in [`load_config`]; this function is then
/// called from read paths (bootstrap projection, `set_settings`) that run far
/// more often than the config loads, and logging from here would spam a
/// warning on every one of those reads for a value that was already reported
/// once. Mirrors `clampTerminalFontSize` in the web's `lib/terminalFont.ts`.
pub fn normalized_terminal_font_size(size: u16) -> u16 {
    if !(MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE).contains(&size) {
        return DEFAULT_TERMINAL_FONT_SIZE;
    }
    size
}

/// Hard ceiling on `ui.attention_grace_seconds` (the settings-PATCH endpoint
/// clamps to this). Five minutes is far longer than any useful "just came
/// back" grace window; `0` ("clear immediately") is a separate, always-allowed
/// value handled by the caller before clamping.
pub const MAX_ATTENTION_GRACE_SECONDS: u64 = 300;

/// Convert a configured `shutdown_timeout_seconds` into the grace `Duration`
/// every shutdown path uses, clamped to [`MAX_SHUTDOWN_TIMEOUT_SECONDS`]. Logs a
/// warning when the configured value is above the ceiling so the operator learns
/// their setting is being capped (and is nudged that the unit is seconds, not
/// milliseconds). Centralized so the TUI quit, the web flip, and `dux server`
/// all derive the grace identically.
pub fn shutdown_grace(seconds: u16) -> std::time::Duration {
    if seconds > MAX_SHUTDOWN_TIMEOUT_SECONDS {
        crate::logger::warn(&format!(
            "shutdown_timeout_seconds = {seconds} exceeds the maximum of \
             {MAX_SHUTDOWN_TIMEOUT_SECONDS}s and is being clamped (the value is in \
             SECONDS, not milliseconds)."
        ));
    }
    std::time::Duration::from_secs(u64::from(seconds.min(MAX_SHUTDOWN_TIMEOUT_SECONDS)))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// LOCAL MODE bind host. `dux server` binds `host:port` (plus the machine's
    /// Tailscale address when `tailscale_enabled`). Must be an IP literal such as
    /// `127.0.0.1` (loopback, the safe default) or `0.0.0.0` (all interfaces);
    /// hostnames are not resolved. Default `127.0.0.1`.
    pub host: String,
    /// LOCAL MODE port. `dux server` and the palette flip bind `host:port` (plus
    /// the machine's Tailscale address when `tailscale_enabled`). Default 8080.
    pub port: u16,
    /// OPT-OUT Tailscale binding. When true, the server also binds the machine's
    /// Tailscale address (100.64.0.0/10) so tailnet devices reach dux over
    /// WireGuard. Detection shells out to `tailscale ip`; when the CLI is missing
    /// or the daemon is down, dux WARNS and serves the configured host only.
    pub tailscale_enabled: bool,
    /// Extra `Host` header values to accept when the request is NOT same-origin.
    /// dux is trusted-local: it always serves on `host:port` (loopback by default)
    /// and accepts same-origin requests. List any additional hostnames a reverse
    /// proxy or tailnet name forwards under (e.g. `box.tailnet.ts.net`) so those
    /// requests are not rejected by the host guard. Empty by default.
    pub allowed_hosts: Vec<String>,
    /// Colored, vite-style console output for `dux server`. One of `"auto"`
    /// (default — color only when stdout is a terminal, `NO_COLOR` is unset, and
    /// `TERM` is not `dumb`), `"always"` (force color), or `"never"` (plain text).
    /// An unrecognized value is treated as `"auto"` with a warning. The TUI flip's
    /// status screen is unaffected — this only governs the `dux server` CLI.
    pub color: String,
    /// Whether `dux server` prints a per-request access log line (method, path,
    /// status, latency) to its console. The `/healthz` probe is always skipped.
    /// Default true. The access log is console-only (never written to `dux.log`),
    /// so piping `dux server`'s stdout captures it.
    pub access_log: bool,
    /// Maximum number of concurrent events (`/ws`) WebSocket connections. This is
    /// the status/changed-files event stream every browser tab opens. Once this
    /// many are live, further upgrade attempts are rejected with HTTP 503 until a
    /// slot frees. Default 32. A value of 0 permanently blocks this connection
    /// class until the server restarts. Changing this requires a server restart to
    /// take effect: the connection-cap semaphore is built at startup and a config
    /// reload cannot resize it.
    pub max_websocket_events_connections: u32,
    /// Maximum number of concurrent agent-PTY WebSocket connections. This is the
    /// embedded-terminal stream for an agent session. Once this many are live,
    /// further upgrade attempts are rejected with HTTP 503 until a slot frees.
    /// Default 32. A value of 0 permanently blocks this connection class until the
    /// server restarts. Changing this requires a server restart to take effect:
    /// the connection-cap semaphore is built at startup and a config reload cannot
    /// resize it.
    pub max_websocket_agent_connections: u32,
    /// Maximum number of concurrent terminal-PTY WebSocket connections. This is the
    /// standalone scratch-terminal stream. Once this many are live, further upgrade
    /// attempts are rejected with HTTP 503 until a slot frees. Default 64. A value
    /// of 0 permanently blocks this connection class until the server restarts.
    /// Changing this requires a server restart to take effect: the connection-cap
    /// semaphore is built at startup and a config reload cannot resize it.
    pub max_websocket_terminal_connections: u32,
    /// Maximum number of concurrent extra-tab PTY WebSocket connections across
    /// ALL agents. Tab sockets draw from THIS pool, not the agent-PTY pool, so a
    /// few agents each showing many tabs cannot 503 every other agent's Main
    /// terminal. Once this many are live, further tab-socket upgrades are rejected
    /// with HTTP 503 until a slot frees. Default 64. A value of 0 permanently
    /// blocks all extra-tab PTY sockets until the server restarts. Changing this
    /// requires a server restart to take effect.
    pub max_websocket_tab_connections: u32,
    /// Maximum concurrent live extra-tab PTY WebSocket connections a SINGLE
    /// agent may hold, checked BEFORE a permit is taken from the shared tab pool
    /// (`max_websocket_tab_connections`). This is a per-agent fairness sub-quota on
    /// top of that pool: it stops one agent showing many tabs from monopolizing the
    /// pool and starving other agents' tabs. Once an agent reaches this many live
    /// tab sockets, further tab-socket upgrades for THAT agent are rejected with
    /// HTTP 503 until one closes. Default 8. A value of 0 permanently blocks all
    /// extra-tab PTY sockets until the server restarts.
    pub max_websocket_tabs_per_agent: u32,
    /// WEB-ONLY display name for this dux instance. Drives the browser tab
    /// `<title>` and the brand wordmark in the web projects pane (the version
    /// line stays directly below it). Set a distinct value per instance (e.g.
    /// "dux #1" or "dux (prod)") to tell several dux tabs apart at a glance.
    /// Default "dux". An empty/whitespace value falls back to "dux" in the UI.
    pub title: String,
    /// WEB-ONLY favicon color for this dux instance, so several dux tabs are easy
    /// to tell apart. Empty (default) keeps the original full-color yellow duck.
    /// Otherwise one of the curated tint colors, which recolors a flat duck
    /// silhouette in the browser tab: violet, blue, sky, cyan, teal, green, amber,
    /// orange, red, pink, rose. Unrecognized values fall back to the default duck.
    pub favicon: String,
    /// Seconds the web server (`dux server`, or the server flipped from the TUI)
    /// waits for agents and companion terminals to exit after SIGTERM on
    /// shutdown, before force-killing (SIGKILL) any stragglers. `0` skips the
    /// wait and force-kills immediately; values above
    /// [`MAX_SHUTDOWN_TIMEOUT_SECONDS`] are clamped. A second Ctrl-c/SIGTERM
    /// during the wait forces an immediate exit. The TUI quit path uses the
    /// top-level `shutdown_timeout_seconds` instead. Default 30.
    pub shutdown_timeout_seconds: u16,
    /// Maximum number of files the web editor's "Search files…" index will
    /// collect in a single flat walk of the worktree. The file TREE is a lazy,
    /// per-directory browser and is never capped; this bounds only the search
    /// index, where an incomplete result on a very large repo (e.g. a built
    /// `target/`) is an acceptable tradeoff for a bounded response. `0`
    /// disables the cap. Default 50000. Takes effect on the next search-index
    /// fetch after a server restart.
    pub search_index_max_files: usize,
    /// Maximum number of `/files/tree` directory listings the web editor may
    /// run concurrently across all sessions. Each listing does one blocking
    /// `read_dir` off the async reactor (`spawn_blocking`); this protects the
    /// server's blocking-thread pool from a burst of tree requests (e.g. many
    /// tabs expanding directories at once) starving other blocking work like
    /// git operations and file reads/writes. A request beyond the limit WAITS
    /// for a free slot rather than being rejected — unlike the
    /// `max_websocket_*_connections` family, this bounds a small, fast unit of
    /// background work, not a long-lived connection. `0` disables the bound
    /// entirely. Default 8. Takes effect on the next server restart.
    pub tree_list_max_concurrency: u32,
    /// Maximum number of release-notes fetches (`GET /api/v1/release-notes`,
    /// behind the app menu's "What's new…") the web server may run
    /// concurrently. Each one is a blocking HTTPS round trip to the GitHub API
    /// run off the async reactor (`spawn_blocking`); this protects the server's
    /// blocking-thread pool from a burst of clicks (or several browser tabs)
    /// starving other blocking work like git operations and file reads. A
    /// request beyond the limit WAITS for a free slot rather than being
    /// rejected, and since the notes are cached with a six-hour TTL the waiter
    /// usually returns immediately from cache. `0` disables the bound entirely.
    /// Default 2. Takes effect on the next server restart.
    pub release_notes_max_concurrency: u32,
    /// Maximum size, in bytes, of a single file dropped onto a terminal or agent
    /// pane in the web UI. Default 104857600 (100 MiB). Set EXPLICITLY on the
    /// upload route, because the web framework's own default of 2 MB would
    /// reject an ordinary screenshot. A file over the cap is refused with a
    /// message saying so and nothing is written. `0` disables file drop
    /// entirely: the route refuses every upload and the browser stops offering
    /// the drop overlay. Read at startup, so changing it needs a server
    /// restart.
    pub file_drop_max_bytes: usize,
    /// Maximum number of dropped-file uploads the web server will have in flight
    /// at once. The permit is taken BEFORE the request body is read (the body is
    /// fully buffered in memory before a handler's first line runs, so taking it
    /// inside the handler would be too late and the memory already spent), which
    /// is what makes this bound total buffered upload memory rather than just
    /// serializing the work. A request beyond the limit WAITS for a free slot
    /// rather than being refused. Default 2, so with the default size cap the
    /// worst case is roughly 200 MiB of buffered upload. `0` clamps to 1 rather
    /// than disabling the bound: a zero-permit semaphore would stall every drop
    /// forever. Read at startup, so changing it needs a server restart.
    pub file_drop_max_concurrency: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub resume_args: Option<Vec<String>>,
    pub resume_wait_timeout_ms: Option<u64>,
    pub install_hint: Option<String>,
    /// Scroll-forwarding policy for the wheel and PgUp/PgDn over this
    /// provider's embedded PTY. Tri-state:
    ///
    /// - `None` (key absent) — auto: forward to the child only when it owns
    ///   the screen and asked for the wheel (alternate screen + mouse
    ///   reporting for the wheel; alternate screen alone for the page keys),
    ///   otherwise scroll dux's own host scrollback. This adapts to apps like
    ///   Claude Code that switch to a fullscreen alt-screen renderer.
    /// - `Some(true)` — always forward scroll and page keys to the child.
    /// - `Some(false)` — never forward; always use dux host scrollback.
    pub forward_scroll: Option<bool>,
    /// How the WEB UI writes a dropped file's path into this provider's prompt.
    /// The raw config string, parsed at use through [`WebDragDropPaste`] so a typo
    /// degrades gracefully instead of failing the whole config load (the
    /// `capabilities.clipboard_passthrough` convention). `None` (key absent)
    /// resolves to [`WebDragDropPaste::Bare`].
    ///
    /// See [`WebDragDropPaste`] for what each form means and which CLI needs which.
    pub web_dragdrop_paste: Option<String>,
}

/// The form a DRAGGED AND DROPPED file's path takes when the WEB UI writes it
/// into a provider's prompt.
///
/// The `web_` prefix on the config key is load-bearing: this affects the browser
/// and nothing else. The terminal UI needs none of it, because dropping a file
/// onto a terminal window there is the host terminal emulator's job, and the file
/// is already on the machine the agent runs on.
///
/// This exists because the receiving end is an agent CLI, not a shell, and the
/// CLIs do not agree on how they read a pasted path. Each one takes the whole
/// pasted string and normalizes it its own way before deciding whether it names
/// a file. The right form is therefore a property of the CLI, which is why it is
/// a per-provider setting a maintainer can extend rather than a rule baked in
/// here.
///
/// WHAT WAS MEASURED. Every row below was produced by RUNNING the CLI's own
/// normalizer over the exact bytes dux sends, not by reading and summarizing it.
/// The measurements are what makes this a table of evidence rather than opinion,
/// so a new row belongs here only once someone has run the new CLI the same way.
///
/// | CLI                 | What it does to a pasted path                                           | Form it needs   |
/// |---------------------|-------------------------------------------------------------------------|-----------------|
/// | Claude Code 2.1.220 | Trims, strips ONE surrounding matching quote pair, unescapes `\X`, then  | `bare`          |
/// |                     | matches the extension. Never splits on whitespace, so a space is        |                 |
/// |                     | harmless bare.                                                          |                 |
/// | OpenCode            | Strips ALL leading and trailing quote characters, resolves `file://`,   | `bare`          |
/// |                     | unescapes `\X`. No shell splitting, so a space is harmless bare.        |                 |
/// | Codex (shlex 1.3.0) | Strips one matching quote pair, resolves `file://`, otherwise runs the   | `single_quoted` |
/// |                     | text through POSIX shell lexing and accepts it ONLY if it comes out as  |                 |
/// |                     | exactly one token. A bare path containing a space is silently ignored.  |                 |
/// | Copilot CLI         | Closed source. NOT verified. Defaulted to `bare`, the do-nothing option | `bare` (guess)  |
/// |                     | and what two of the three verified CLIs want.                           |                 |
///
/// Anything dux does not ship, including a provider a user adds themselves, gets
/// `bare` for the same reason.
///
/// WHAT IS KNOWN TO FAIL, which is the more useful half of the table:
///
/// - `single_quoted` on a path containing an APOSTROPHE breaks Claude Code. POSIX
///   writes an embedded apostrophe as close-escape-reopen, and Claude Code's
///   unescape step then collapses that into three consecutive apostrophes. Run
///   against exactly what dux produced, a real `/home/p/Bob's app/shot.png` came
///   back with three apostrophes in the middle of it, naming nothing.
/// - ANY form carrying a BACKSLASH is mangled by Claude Code's unescape step. That
///   covers `backslash_escaped` outright, and also a path that simply has a
///   backslash in its name, whatever form it is sent in. The unescape eats it.
///
/// Both are properties of the receiving tool, not bugs in dux, and no workaround
/// should be attempted from this side: dux sends the correct bytes and the CLI
/// rewrites them.
///
/// GETTING IT WRONG IS NOT USUALLY A BREAKAGE. The normal symptom is that the file
/// is not attached automatically and its path is left in the prompt as ordinary
/// text, which the user can still work with.
///
/// ADDING A FORM. The set is open by construction: a new form is one more variant
/// plus one more arm in [`WebDragDropPaste::parse`], [`WebDragDropPaste::as_str`]
/// and the web's `pastePayload`, with no rework anywhere else. One candidate is
/// deliberately ABSENT: a `file://` URL. Codex and OpenCode both resolve one, but
/// whether Claude Code does on its paste path is UNVERIFIED, and a form should not
/// ship on an unmeasured assumption. Measure it and it can be added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebDragDropPaste {
    /// The path exactly as it is on disk: nothing added, nothing escaped.
    ///
    /// What Claude Code and OpenCode want. Both take the whole pasted string and
    /// never split on whitespace, so a space needs no protection. This is the
    /// default for every provider not listed otherwise.
    #[default]
    Bare,
    /// Wrapped in single quotes, with an embedded apostrophe escaped the POSIX way
    /// (close the quote, escape the apostrophe, reopen the quote).
    ///
    /// What Codex wants. It lexes the pasted text with POSIX shell rules and
    /// accepts it only if it comes out as exactly one token, so a bare path
    /// containing a space is silently ignored.
    SingleQuoted,
    /// Wrapped in double quotes, with an embedded double quote and backslash
    /// escaped.
    ///
    /// Also lexes to one token for Codex, and both other CLIs strip a surrounding
    /// pair. Offered because a future CLI may prefer it, and because a path full of
    /// apostrophes is cleaner this way than through the close-escape-reopen dance.
    DoubleQuoted,
    /// No quotes, with shell-significant characters escaped by a backslash, so
    /// `My File.png` goes out as `My\ File.png`.
    ///
    /// Codex accepts it (it lexes to one token) and OpenCode unescapes it back. It
    /// is also what several real terminal emulators emit when you drop a file on
    /// them, so it is the closest thing here to "what a normal terminal does".
    /// Note the failure above: Claude Code's unescape step mangles it.
    BackslashEscaped,
}

impl WebDragDropPaste {
    /// Parse a config string into a mode, returning `None` for an unrecognized
    /// value so the caller decides whether to warn. Never warns itself, so a
    /// per-paste path can call it freely.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bare" => Some(Self::Bare),
            "single_quoted" => Some(Self::SingleQuoted),
            "double_quoted" => Some(Self::DoubleQuoted),
            "backslash_escaped" => Some(Self::BackslashEscaped),
            _ => None,
        }
    }

    /// The warning text for one unrecognized value, or `None` when the value is a
    /// form dux knows.
    ///
    /// Returned rather than logged so the message itself is testable: a warning
    /// that only ever reaches a file cannot be asserted on without racing the
    /// process-wide logger, and "warns once per load" is exactly the property
    /// worth pinning.
    pub fn unknown_value_warning(provider: &str, s: &str) -> Option<String> {
        if Self::parse(s).is_some() {
            return None;
        }
        Some(format!(
            "unknown providers.{provider}.web_dragdrop_paste value {s:?}; falling back to \
             \"bare\" (valid: bare, single_quoted, double_quoted, backslash_escaped)"
        ))
    }

    /// Parse a config string, falling back to [`WebDragDropPaste::Bare`] with a logged
    /// warning on an unrecognized value. Call this at config load/reload (once),
    /// not per paste, so a typo is surfaced without spamming the log; the paste
    /// path uses the non-warning [`WebDragDropPaste::parse`].
    pub fn from_config_str(provider: &str, s: &str) -> Self {
        if let Some(warning) = Self::unknown_value_warning(provider, s) {
            crate::logger::warn(&warning);
        }
        Self::parse(s).unwrap_or(Self::Bare)
    }

    /// The canonical lowercase name. This is what goes in `config.toml` and what
    /// is projected into the web bootstrap document, so both surfaces agree.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bare => "bare",
            Self::SingleQuoted => "single_quoted",
            Self::DoubleQuoted => "double_quoted",
            Self::BackslashEscaped => "backslash_escaped",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "new_project_id")]
    pub id: String,
    pub path: String,
    pub name: Option<String>,
    pub default_provider: Option<String>,
    pub leading_branch: Option<String>,
    pub auto_reopen_agents: Option<bool>,
    pub startup_command: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

pub fn new_project_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub left_width_pct: u16,
    pub right_width_pct: u16,
    pub terminal_pane_height_pct: u16,
    pub empty_project_separator_min_projects: u16,
    pub staged_pane_height_pct: u16,
    pub commit_pane_height_pct: u16,
    pub agent_scrollback_lines: usize,
    /// Maximum number of tabs a single agent may have, counting the session-slot tab
    /// (so 20 means the session-slot tab plus up to 19 ephemeral extra tabs). The "+"
    /// affordance disables at the cap. `0` means use the default (20); values
    /// above the internal ceiling are clamped with a warning. Default 20.
    pub agent_tabs_max: u16,
    /// Seconds before a transient status-line message (a success/info
    /// confirmation) auto-clears. In the TUI's status line, busy/pending and
    /// warning/error messages are unaffected: they persist until replaced. The
    /// web's toasts use this as a base for every tone (warning 2x, error 4x),
    /// because a toast you have to click away is friction a status line is not.
    /// 0 disables auto-clear entirely.
    pub status_clear_seconds: u16,
    pub branch_sync_interval: u16,
    pub show_diff_line_numbers: bool,
    pub diff_tab_width: u16,
    pub github_integration: bool,
    /// Seconds between blind GitHub PR-status safety polls. Most updates are
    /// event-driven (a branch push, or focusing an agent), so this is only the
    /// backstop for changes made on GitHub itself. `0` disables the blind poll
    /// entirely (updates then come only from those events). Clamped to
    /// [`MAX_PR_POLL_INTERVAL_SECONDS`].
    pub pr_poll_interval_seconds: u16,
    /// Whether selecting text in the web terminal auto-copies it to the
    /// clipboard (X11-style "highlight to copy"). Changing it from the web's
    /// Preferences dialog persists the new value here. Web-only behavior.
    pub copy_on_select: bool,
    /// A font name installed on the VIEWING device, placed ahead of dux's
    /// bundled web terminal font stack so the bundled faces still fill in
    /// glyphs it lacks (box drawing, blocks, braille, arrows, powerline).
    /// Empty (the default) means use the bundled stack only. Web UI only; the
    /// TUI always uses the host terminal's own font. Changing it from the
    /// web's Preferences dialog persists the new value here.
    pub terminal_font_family: String,
    /// The web terminal's font size in pixels. Valid range
    /// [`MIN_TERMINAL_FONT_SIZE`, `MAX_TERMINAL_FONT_SIZE`]; a value outside
    /// it is reset to [`DEFAULT_TERMINAL_FONT_SIZE`] (with a warning) when the
    /// bootstrap document is built, so a bad config value can never reach the
    /// browser's terminal. Web UI only. Changing it from the web's
    /// Preferences dialog persists the new value here.
    pub terminal_font_size: u16,
    /// Whether the web UI's mobile terminal shows the compose bar: a buffered
    /// text box below the accessory-bar keys where the phone keyboard's
    /// autocorrect/swipe input work, with a Send button that delivers the
    /// message plus a submitting Enter in one write. While enabled, tapping
    /// the terminal focuses the compose box instead of the raw terminal input.
    /// When false, the bar is hidden and a tap types directly into the
    /// terminal (the pre-compose-bar behavior). Changing it from the web's
    /// Preferences dialog persists the new value here. Web-only behavior.
    pub compose_bar: bool,
    /// Whether the web UI's mobile terminal screens show the top bar (the
    /// back-chevron header with the branch crumb and actions, plus the agent
    /// tab strip below it). Set to false to hide it and give those rows back
    /// to the terminal. Hidden bars can be restored from the show-bars
    /// button below the terminal or from the web UI's Preferences dialog.
    /// Web-only behavior; the hub and Changes screens are unaffected.
    pub mobile_top_bar: bool,
    /// Whether the web UI's mobile terminal screens show the terminal-keys
    /// accessory bar (Esc, Tab, Ctrl, Alt, the arrows and paging keys). Set
    /// to false to hide it and give those rows back to the terminal. Hidden
    /// bars can be restored from the show-bars button below the terminal or
    /// from the web UI's Preferences dialog. Web-only behavior.
    pub mobile_accessory_bar: bool,
    /// Directory, RELATIVE to the agent's worktree, that a file dropped or
    /// pasted onto an AGENT pane is saved into. An absolute path, a `..`
    /// traversal, or an empty value degrades to [`DEFAULT_UPLOAD_DIRECTORY`]
    /// with one warning at load (see [`upload_directory_load_warning`]).
    ///
    /// A TERMINAL pane is deliberately not covered: a file dropped on a
    /// terminal still lands in the directory that terminal is actually in,
    /// because a shell that has been `cd`'d somewhere is showing the user where
    /// they are working.
    ///
    /// This pair lives on `[ui]` rather than in a section of its own because a
    /// top-level `[uploads]` section would imply the feature exists in the TUI
    /// too, and it does not: dropping a file on a terminal window in the TUI is
    /// the host emulator's job, and the in-browser editor is web-only. `[ui]` is
    /// already the home for web-only preferences (`compose_bar`,
    /// `copy_on_select`), and the `upload_` prefix groups the pair the way
    /// `terminal_font_family` / `terminal_font_size` already do.
    pub upload_directory: String,
    /// Keep a `.gitignore` holding a single `*` in the upload directory, so the
    /// uploads (and that file itself) are invisible to git. Attempted on every
    /// upload rather than only on the creation of the directory: it costs one
    /// exclusive-create syscall, and it puts the file back for a directory that
    /// was created while this setting was off, or whose ignore file was
    /// deleted. An existing `.gitignore` is never touched, whatever it holds,
    /// and dux never writes to `.git/info/exclude`. Web-only behavior, as
    /// `upload_directory` is.
    pub upload_write_gitignore: bool,
    /// Seconds the attention indicators stay visible after dux regains your
    /// attention, before the focused agent's needs-attention flag clears.
    /// Applies when you return to the dux browser tab (web UI) and when your
    /// terminal window regains focus (TUI). Gives you time to see which
    /// agent(s) wanted you before the indicator vanishes. `0` clears
    /// immediately (the pre-grace behavior). TUI note: requires a terminal
    /// that reports focus; under tmux, set `focus-events on`. Terminals that
    /// never report focus keep the old behavior.
    pub attention_grace_seconds: u64,
    pub auto_reopen_agents: bool,
    /// Show the right-hand Changes pane (the changed-files list) by default.
    /// Toggling it from the TUI's command palette, the web's Changes actions
    /// menu, or the web's Preferences dialog persists the new value here
    /// immediately; it is not a per-session override.
    pub show_changes_pane: bool,
    /// Always show the agent tab strip, even when a session has only one tab.
    /// Default false shows the strip only once a session has two or more
    /// tabs. Changing it from the TUI's command palette or the web's
    /// Preferences dialog persists the new value here immediately; it is a
    /// shared preference, not a per-session override.
    pub always_show_tab_strip: bool,
    /// Show an indicator when an agent asks for attention (a permission prompt,
    /// a finished turn). Detected from the agent's terminal notifications and
    /// bell. When false, no attention glyph/dot/tab-title/favicon cue is shown on
    /// either surface. Default true.
    pub attention_indicator: bool,
    /// Also treat a plain terminal bell as an attention request. The bell is the
    /// most compatible signal (Codex falls back to it; Claude Code emits it in
    /// terminal_bell mode) but can occasionally ring for mundane reasons, so this
    /// switch turns it off independently of `attention_indicator`. Default true.
    pub attention_on_bell: bool,
    /// Suppress the AUTOMATIC first-run welcome screen. Default false (the
    /// screen shows once, on the very first launch of a fresh install). Opening
    /// the welcome screen deliberately still works when this is true: the flag
    /// governs what dux does on its own, not what the user asks for.
    pub disable_automated_welcome_screen: bool,
    /// Suppress the AUTOMATIC what's-new screen after an upgrade, AND the
    /// startup fetch of the release notes that feeds it (so nothing touches the
    /// network on launch). Default false. Opening the release notes deliberately
    /// still works and ALWAYS fetches: this flag governs the automatic screen,
    /// not what that screen is allowed to contain.
    pub disable_release_notes: bool,
    pub pr_banner_position: String,
    /// The web agent-list sort mode, persisted so a chosen order (and the manual
    /// drag order it enables) survives restarts and is shared across clients:
    /// "active" (working/attention float up, the default), "updated", "created",
    /// "name", or "manual" (the raw persisted order, enabled by drag-reorder).
    /// Web-only today; the TUI keeps its one-shot sort palette commands.
    pub agent_sort: String,
    pub theme: String,
}

/// Terminal capability controls: what identity dux presents to an agent, and
/// which of the escape sequences the agent emits (notifications, progress,
/// clipboard, hyperlinks) dux forwards to the host terminal or the browser.
///
/// `terminal_identity` and `clipboard_passthrough` are stored as strings and
/// parsed at use so a typo degrades gracefully (warn and fall back) instead of
/// failing the whole config load, mirroring the theme/color-config convention.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CapabilitiesConfig {
    /// How dux presents itself to the agent: `auto` (mirror the host terminal in
    /// the TUI, force ghostty on the headless server), `mirror`, `ghostty`,
    /// `kitty`, `iterm2`, or `none` (inherit dux's own env unchanged).
    pub terminal_identity: String,
    /// Master switch for forwarding an agent's notification/progress/clipboard
    /// sequences OUTWARD of dux. In the TUI that is the whole host forward:
    /// false sends the host terminal nothing. On the web the only thing an agent
    /// forwards outward is the OSC 52 clipboard write, so false seals that.
    /// It does NOT govern browser desktop notifications; `web_notifications`
    /// alone does.
    pub passthrough: bool,
    /// Which agents' OSC 52 clipboard-SET sequences reach the clipboard, on BOTH
    /// surfaces: `focused` (only the tab you are viewing), `always` (any tab), or
    /// `off`. Requires `passthrough = true` on both surfaces. Clipboard READ
    /// queries are never forwarded on either surface.
    pub clipboard_passthrough: String,
    /// Render OSC 8 hyperlinks as clickable (TUI host embed and web click handler).
    pub hyperlinks: bool,
    /// Bridge agent notification sequences to a browser Notification (WEB only).
    /// The only switch over those: `passthrough` does not gate them, so sealing
    /// the clipboard leaves browser notifications working. No effect on the TUI,
    /// whose host-terminal notifications are governed by `passthrough` alone.
    pub web_notifications: bool,
}

impl Default for CapabilitiesConfig {
    fn default() -> Self {
        Self {
            terminal_identity: "auto".to_string(),
            passthrough: true,
            clipboard_passthrough: "focused".to_string(),
            hyperlinks: true,
            web_notifications: true,
        }
    }
}

/// Default `ui.upload_directory`: inside the worktree (so a CLI that restricts
/// reads to its workspace can still open the file), and self-ignoring.
pub const DEFAULT_UPLOAD_DIRECTORY: &str = ".dux/uploads";

/// The most a configured `ui.upload_directory` may measure, in bytes.
///
/// This is the platform's own `PATH_MAX` (4096 on Linux, 1024 on macOS), and it
/// is a conservative bound rather than an exact one: the value is only the
/// RELATIVE tail of a path that also carries a worktree in front of it, so a
/// relative tail that already fills `PATH_MAX` cannot fit under any worktree
/// whatsoever. How much of the remaining room a particular worktree leaves is
/// not knowable at load, and is left to the syscall.
pub const MAX_UPLOAD_DIRECTORY_BYTES: usize = libc::PATH_MAX as usize;

/// Why a configured `ui.upload_directory` cannot be used, or `None` when it is
/// usable. Pure, and phrased as a sentence fragment the warning completes.
///
/// **This is a check on the SHAPE of the path and nothing else.** It proves the
/// path is relative and walks downward through named components only, so no
/// value dux accepts can NAME somewhere outside the worktree. It cannot prove
/// where the path RESOLVES to: a symlinked component could still point out of
/// the tree, and no amount of string inspection would see it. That is enforced
/// where it can actually be enforced, at creation time, by walking the path one
/// component at a time from a pinned worktree handle with `O_NOFOLLOW`, which
/// refuses a symlink instead of following it (see
/// `crate::file_drop::open_uploads_dir`).
///
/// The last three refusals are a different kind, and they are here for a
/// different reason: they name a shape the FILESYSTEM will refuse. A value
/// holding a control character (reachable from TOML through a `\n` escape) used
/// to pass, and the directory really was created, but every drop into it then
/// failed; a NUL failed with an opaque `Invalid argument`; an over-long one
/// failed with `File name too long`. All three per drop, in a message about the
/// wrong subject. Refusing them at load is what the warn-once-and-degrade design
/// exists to do.
fn upload_directory_rejection(configured: &str) -> Option<&'static str> {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return Some("it is empty");
    }
    if trimmed.contains('\0') {
        return Some("it contains a null byte, which no filesystem can store");
    }
    if trimmed.chars().any(char::is_control) {
        return Some(
            "it contains a control character, which cannot be typed back and would scramble \
             any terminal the path is printed to",
        );
    }
    if trimmed.len() > MAX_UPLOAD_DIRECTORY_BYTES {
        return Some("it is longer than this platform allows a path to be");
    }
    let path = std::path::Path::new(trimmed);
    let mut components = 0usize;
    for component in path.components() {
        match component {
            std::path::Component::Normal(name) => {
                if name.len() > crate::file_drop::FALLBACK_NAME_MAX_BYTES {
                    return Some(
                        "one of its path components is longer than a filesystem will accept",
                    );
                }
                components += 1;
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Some("it is an absolute path, and uploads are stored inside the worktree");
            }
            std::path::Component::ParentDir => {
                return Some("it contains a \"..\" component, which would leave the worktree");
            }
            // `.` names the directory it sits in, so it changes nothing and is
            // simply dropped. Refusing it was inconsistent as well as useless:
            // MEASURED, `Path::components()` preserves a CurDir only in LEADING
            // position, so `uploads/./x` was already accepted and normalized
            // while `./uploads`, an ordinary way to write a relative path, was
            // turned away. A value that is NOTHING but `.` still falls to the
            // "names no directory" refusal below.
            std::path::Component::CurDir => {}
        }
    }
    if components == 0 {
        return Some("it names no directory");
    }
    None
}

/// Normalize a configured `ui.upload_directory` into the relative path dux will
/// actually create: the configured value with its components rejoined, or
/// [`DEFAULT_UPLOAD_DIRECTORY`] when the configured one is unusable.
///
/// Deliberately PURE: it does not log. The on-disk value is warned about and
/// corrected exactly once, at load, in [`load_config`]; this is then called from
/// read paths (every upload resolves its destination through it) that run far
/// more often than the config loads. Same split as
/// [`normalized_terminal_font_size`].
pub fn normalized_upload_directory(configured: &str) -> String {
    if upload_directory_rejection(configured).is_some() {
        return DEFAULT_UPLOAD_DIRECTORY.to_string();
    }
    // NORMAL components only. Anything a usable value can still hold at this
    // point is a `.`, which names the directory it sits in and so contributes
    // nothing to the walk that creates the path.
    std::path::Path::new(configured.trim())
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The warning [`load_config`] emits for an unusable `ui.upload_directory`, or
/// `None` for a usable one. Split out from the logging so the message can be
/// asserted directly, mirroring [`terminal_font_size_load_warning`].
pub fn upload_directory_load_warning(configured: &str) -> Option<String> {
    let reason = upload_directory_rejection(configured)?;
    Some(format!(
        "ui.upload_directory = {configured:?} cannot be used because {reason}. Falling back \
         to the default of {DEFAULT_UPLOAD_DIRECTORY:?}."
    ))
}

/// The parsed form of `capabilities.clipboard_passthrough`. Stored as a string in
/// config (so a typo degrades gracefully rather than failing the whole load) and
/// parsed at use, mirroring [`crate::term_identity::TerminalIdentityMode`]. Both
/// surfaces normalize through this so the TUI host forward and the web browser
/// write agree on the mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardPassthroughMode {
    /// Forward a clipboard SET only from the tab the user is currently viewing.
    Focused,
    /// Forward a clipboard SET from any tab, foreground or background.
    Always,
    /// Never forward a clipboard SET.
    Off,
}

impl ClipboardPassthroughMode {
    /// Parse a config string into a mode, returning `None` for an unrecognized
    /// value so the caller can decide whether to warn. Never warns itself, so the
    /// per-tick forward path can call it freely.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "focused" => Some(Self::Focused),
            "always" => Some(Self::Always),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// Parse a config string, falling back to [`ClipboardPassthroughMode::Focused`]
    /// with a logged warning on an unrecognized value. Call this at config
    /// load/reload (once), NOT per tick, so a typo is surfaced without spamming the
    /// log; the forward path uses the non-warning [`ClipboardPassthroughMode::parse`].
    pub fn from_config_str(s: &str) -> Self {
        match Self::parse(s) {
            Some(mode) => mode,
            None => {
                crate::logger::warn(&format!(
                    "unknown capabilities.clipboard_passthrough value {s:?}; falling back to \
                     \"focused\" (valid: focused, always, off)"
                ));
                Self::Focused
            }
        }
    }

    /// The canonical lowercase name, used to project the normalized mode into the
    /// web bootstrap document so both surfaces agree.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Focused => "focused",
            Self::Always => "always",
            Self::Off => "off",
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        let start_directory = home::home_dir().map(|p| p.to_string_lossy().to_string());
        Self {
            provider: "claude".to_string(),
            start_directory,
            enable_randomized_pet_name_by_default: false,
            pull_before_creating_agent_by_default: true,
            copy_uncommitted_changes_by_default: true,
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        let commands = default_provider_commands()
            .into_iter()
            .map(|(name, config)| (name.to_string(), config))
            .collect();
        Self { commands }
    }
}

impl ProviderCommandConfig {
    pub fn interactive_args(&self, resume_session: bool) -> Vec<String> {
        let mut args = self.args.clone();
        if resume_session
            && let Some(resume_args) = self.resume_args.as_deref().filter(|args| !args.is_empty())
        {
            args.extend(resume_args.iter().cloned());
        }
        args
    }

    pub fn supports_session_resume(&self) -> bool {
        self.resume_args
            .as_ref()
            .map(|args| !args.is_empty())
            .unwrap_or(false)
    }

    /// The effective [`WebDragDropPaste`] for this provider: the configured value when
    /// it names a form dux knows, and [`WebDragDropPaste::Bare`] when the key is absent
    /// or misspelled. Silent, because a warning belongs at config load and not on
    /// every paste; `load_config` emits it once (see `warn_on_unknown_web_dragdrop_paste_forms`).
    pub fn resolved_web_dragdrop_paste(&self) -> WebDragDropPaste {
        self.web_dragdrop_paste
            .as_deref()
            .and_then(WebDragDropPaste::parse)
            .unwrap_or_default()
    }

    /// The FILE NAME of the command this provider runs, which is the only thing
    /// in a `[providers.<name>]` block that identifies which CLI is on the other
    /// end of a paste.
    ///
    /// The block's NAME does not, and treating it as though it did was a real
    /// defect: a provider's name is free text the user chooses, so
    /// `[providers.myagent] command = "codex"` is a real Codex and
    /// `[providers.codex] command = "something-else"` is not. Anything keyed by
    /// the name therefore answered for the wrong CLI in both directions at once.
    /// The web's per-CLI paste-length table is keyed by this instead.
    ///
    /// The FILE NAME rather than the whole string, because `command` may be a
    /// full path (`/usr/local/bin/codex`), a `~`-relative one, or a bare name
    /// found on `PATH`, and all three name the same CLI. Argument-carrying
    /// wrappers (`command = "npx"`, `command = "mise"`) are deliberately NOT
    /// unwrapped: what they finally exec is not knowable from config, so they
    /// resolve to the wrapper and fall into the "no entry, no limit" case, which
    /// withholds nothing.
    ///
    /// Falls back to the whole string when there is no file name to take (an
    /// empty command, or one that is nothing but separators), so the answer is
    /// never silently empty.
    pub fn command_file_name(&self) -> String {
        std::path::Path::new(&self.command)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.command.clone())
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            path: "dux.log".to_string(),
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            command: default_terminal_command(),
            args: default_terminal_args(),
        }
    }
}

impl Default for StartupCommandTerminalConfig {
    fn default() -> Self {
        Self {
            command: "$SHELL".to_string(),
            args: vec!["-l".to_string(), "-c".to_string()],
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            default: "cursor".to_string(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            tailscale_enabled: true,
            allowed_hosts: Vec::new(),
            color: "auto".to_string(),
            access_log: true,
            max_websocket_events_connections: DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
            max_websocket_agent_connections: DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
            max_websocket_terminal_connections: DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
            max_websocket_tab_connections: DEFAULT_MAX_WEBSOCKET_TAB_CONNECTIONS,
            max_websocket_tabs_per_agent: DEFAULT_MAX_WEBSOCKET_TABS_PER_AGENT,
            title: "dux".to_string(),
            favicon: String::new(),
            shutdown_timeout_seconds: DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
            search_index_max_files: DEFAULT_SEARCH_INDEX_MAX_FILES,
            tree_list_max_concurrency: DEFAULT_TREE_LIST_MAX_CONCURRENCY,
            release_notes_max_concurrency: DEFAULT_RELEASE_NOTES_MAX_CONCURRENCY,
            file_drop_max_bytes: DEFAULT_FILE_DROP_MAX_BYTES,
            file_drop_max_concurrency: DEFAULT_FILE_DROP_MAX_CONCURRENCY,
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            // These must stay equal to the `UiConfig` literal inside
            // `Config::default()`. `[ui]` carries `#[serde(default)]`, so a config
            // whose `[ui]` table omits a width fills it from here, while a fresh
            // install writes the value the canonical template renders from
            // `Config::default()`. When the two disagree the same setting has two
            // defaults depending on how the user arrived, and `dux config diff`
            // reports a width the user never wrote as changed.
            left_width_pct: 20,
            right_width_pct: 23,
            terminal_pane_height_pct: 35,
            empty_project_separator_min_projects: 5,
            staged_pane_height_pct: 50,
            commit_pane_height_pct: 40,
            agent_scrollback_lines: 10_000,
            agent_tabs_max: DEFAULT_AGENT_TABS_MAX,
            status_clear_seconds: 6,
            branch_sync_interval: 30,
            show_diff_line_numbers: false,
            diff_tab_width: 4,
            github_integration: true,
            pr_poll_interval_seconds: DEFAULT_PR_POLL_INTERVAL_SECONDS,
            copy_on_select: true,
            terminal_font_family: String::new(),
            terminal_font_size: DEFAULT_TERMINAL_FONT_SIZE,
            compose_bar: true,
            mobile_top_bar: true,
            mobile_accessory_bar: true,
            upload_directory: DEFAULT_UPLOAD_DIRECTORY.to_string(),
            upload_write_gitignore: true,
            attention_grace_seconds: 3,
            auto_reopen_agents: false,
            show_changes_pane: true,
            always_show_tab_strip: false,
            attention_indicator: true,
            attention_on_bell: true,
            disable_automated_welcome_screen: false,
            disable_release_notes: false,
            pr_banner_position: "bottom".to_string(),
            agent_sort: "active".to_string(),
            theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
        }
    }
}

impl ProvidersConfig {
    pub fn get(&self, name: &str) -> Option<&ProviderCommandConfig> {
        self.commands.get(name)
    }

    pub fn ensure_defaults(&mut self) {
        for (name, config) in default_provider_commands() {
            match self.commands.entry(name.to_string()) {
                indexmap::map::Entry::Vacant(entry) => {
                    entry.insert(config);
                }
                indexmap::map::Entry::Occupied(mut entry) => {
                    if entry.get().resume_args.is_none() {
                        entry.get_mut().resume_args = config.resume_args;
                    }
                    if entry.get().resume_wait_timeout_ms.is_none() {
                        entry.get_mut().resume_wait_timeout_ms = config.resume_wait_timeout_ms;
                    }
                    // A config written before `web_dragdrop_paste` existed has the key
                    // absent, and absent resolves to `bare`. That is wrong for
                    // codex, so fill in the shipped form here rather than letting
                    // an old config silently keep the do-nothing one. An explicit
                    // value is left alone: config wins for explicit preferences.
                    if entry.get().web_dragdrop_paste.is_none() {
                        entry.get_mut().web_dragdrop_paste = config.web_dragdrop_paste;
                    }
                }
            }
        }
    }
}

pub fn default_terminal_command() -> String {
    env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

pub fn default_terminal_args() -> Vec<String> {
    // Launch as a login shell so the user's profile, aliases, and prompt
    // are loaded. The -l flag is supported by bash, zsh, fish, dash, and
    // all POSIX shells.
    vec!["-l".to_string()]
}

/// The providers dux ships, and every default that goes with them.
///
/// `web_dragdrop_paste` here is the MEASURED result of running each CLI's own path
/// normalizer over the bytes dux sends. The table of what each one does, and the
/// caveat that Copilot's value is a guess because it is closed source, lives on
/// [`WebDragDropPaste`]. Read it before changing a value here.
pub fn default_provider_commands() -> [(&'static str, ProviderCommandConfig); 4] {
    [
        (
            "claude",
            ProviderCommandConfig {
                command: "claude".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: None,
                install_hint: Some("curl -fsSL https://claude.ai/install.sh | bash".to_string()),
                forward_scroll: None,
                // Measured: strips one quote pair then unescapes, so quoting
                // buys nothing and corrupts an apostrophe.
                web_dragdrop_paste: Some(WebDragDropPaste::Bare.as_str().to_string()),
            },
        ),
        (
            "codex",
            ProviderCommandConfig {
                command: "codex".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["resume".to_string(), "--last".to_string()]),
                resume_wait_timeout_ms: None,
                install_hint: Some("brew install --cask codex".to_string()),
                forward_scroll: None,
                // Measured: falls back to POSIX shell lexing and accepts only a
                // single token, so a bare path with a space fails silently.
                web_dragdrop_paste: Some(WebDragDropPaste::SingleQuoted.as_str().to_string()),
            },
        ),
        (
            "opencode",
            ProviderCommandConfig {
                command: "opencode".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: Some(3_000),
                install_hint: Some("curl -fsSL https://opencode.ai/install | bash".to_string()),
                forward_scroll: None,
                // Measured: strips quote characters and never splits on a space.
                web_dragdrop_paste: Some(WebDragDropPaste::Bare.as_str().to_string()),
            },
        ),
        (
            "copilot",
            ProviderCommandConfig {
                command: "copilot".to_string(),
                args: Vec::new(),
                // Copilot's --continue resumes the most recent session
                // globally, not scoped to the current working directory.
                // Unlike claude/codex/opencode, there is no flag
                // to limit resume to the CWD, so we disable it.
                resume_args: None,
                resume_wait_timeout_ms: None,
                install_hint: Some("curl -fsSL https://gh.io/copilot-install | bash".to_string()),
                forward_scroll: None,
                // NOT measured: Copilot CLI is closed source. `bare` is the
                // do-nothing option and what two of the three verified CLIs want.
                web_dragdrop_paste: Some(WebDragDropPaste::Bare.as_str().to_string()),
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// DuxPaths: canonical locations for runtime files
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DuxPaths {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub sessions_db_path: PathBuf,
    pub worktrees_root: PathBuf,
    /// Path to the lockfile that enforces a single dux instance per
    /// config directory. Contains the PID of the holder.
    pub lock_path: PathBuf,
}

impl DuxPaths {
    pub fn discover() -> Result<Self> {
        let root = resolve_root(
            env::var_os("DUX_HOME"),
            home::home_dir(),
            env::var_os("XDG_CONFIG_HOME"),
        )?;
        Ok(Self {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        })
    }

    /// Create the config root and the worktrees directory.
    ///
    /// The ROOT is owner-only (`0700`), and that is the load-bearing part of
    /// dux's file permissions rather than a nicety: it holds `config.toml`,
    /// `sessions.sqlite3` (which mirrors the same per-project `env` map that
    /// made the config file `0600`), the sqlite sidecars SQLite creates itself
    /// at runtime with no mode dux can choose, and `dux.log`. A directory
    /// another local user cannot search settles all of them at once.
    ///
    /// It TIGHTENS on every startup, not only on first creation, because every
    /// existing installation already has a `0755` root and a change that only
    /// applied to new ones would reach nobody. The tightening only clears group
    /// and other bits, so it is idempotent and cannot lock the owner out.
    ///
    /// The WORKTREES directory is deliberately left at the umask default. Those
    /// are the user's own checkouts, opened in their own editor, and the mode
    /// dux found is the mode dux leaves.
    ///
    /// Do not repeat the older reason for this, that the checkouts may be
    /// "shared on purpose". They sit inside a `0700` root now, so another local
    /// user cannot search their way in whatever the worktrees directory itself
    /// says, and on-purpose sharing through this path is no longer possible.
    /// Anyone who really was sharing a worktree with another local account lost
    /// that when the root was tightened, and the way back is to put the
    /// worktrees somewhere outside the config root rather than to loosen the
    /// root. The mode is still preserved, for the honest reason that it is the
    /// user's own checkout and not dux's file to relabel.
    pub fn ensure_dirs(&self) -> Result<()> {
        crate::file_modes::create_private_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        // `config.toml` too, and it is the file the promise mattered most for
        // since it holds `[env]` tokens. Nothing used to tighten it. It reached
        // `0600` only because `write_config_atomic` creates its temp at `0600`
        // and renames over the original, and that runs on first creation or on
        // a save that actually changed the document, so a config chmod'd to
        // `0644` by hand stayed `0644` for good while the log and the database
        // were corrected on every open. A missing file is not an error here and
        // is not created, and the pass only ever clears group and other bits, so
        // a deliberate `0400` survives.
        crate::file_modes::restrict_to_owner_best_effort(&self.config_path, "config file");
        fs::create_dir_all(&self.worktrees_root)
            .with_context(|| format!("failed to create {}", self.worktrees_root.display()))?;
        Ok(())
    }
}

pub fn resolve_root(
    dux_home: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
    xdg_config_home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dux_home) = dux_home.map(PathBuf::from) {
        if dux_home.is_absolute() {
            return Ok(dux_home);
        }
        bail!(
            "DUX_HOME must be an absolute path, got: {}",
            dux_home.display()
        );
    }

    let home = home.ok_or_else(|| anyhow!("failed to determine user home directory"))?;
    Ok(discover_root(&home, xdg_config_home))
}

pub fn discover_root(home: &Path, xdg_config_home: Option<std::ffi::OsString>) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = xdg_config_home;
        home.join(".dux")
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = xdg_config_home.map(PathBuf::from)
            && xdg.is_absolute()
        {
            return xdg.join("dux");
        }
        home.join(".config").join("dux")
    }
}

// ---------------------------------------------------------------------------
// Env/path helpers
// ---------------------------------------------------------------------------

/// Expand environment variables (`$VAR`, `${VAR}`) in a config string.
/// Returns `None` when variable syntax is invalid.
pub fn expand_env_vars(raw: &str) -> Option<String> {
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next();
            }
            let mut var_name = String::new();
            while let Some(&c) = chars.peek() {
                if braced {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                } else if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                var_name.push(c);
                chars.next();
            }
            if var_name.is_empty() || !is_valid_var_name(&var_name) {
                return None;
            }
            match std::env::var(&var_name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    result.push('$');
                    if braced {
                        result.push('{');
                    }
                    result.push_str(&var_name);
                    if braced {
                        result.push('}');
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    Some(result)
}

pub fn resolve_project_env(env: &BTreeMap<String, String>) -> Result<Vec<(String, String)>> {
    let mut resolved = Vec::with_capacity(env.len());
    for (name, value) in env {
        validate_project_env_name(name)?;
        let expanded = expand_env_vars(value)
            .ok_or_else(|| anyhow!("environment variable {name} has invalid expansion syntax"))?;
        if expanded.contains('\0') {
            bail!("environment variable {name} contains a NUL byte");
        }
        resolved.push((name.clone(), expanded));
    }
    Ok(resolved)
}

pub fn resolve_agent_env(
    global_env: &BTreeMap<String, String>,
    project_env: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>> {
    let mut merged = global_env.clone();
    merged.extend(project_env.clone());
    resolve_project_env(&merged)
}

pub fn project_env_to_lines(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_project_env_lines(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            bail!("line {} must use KEY=value syntax", index + 1);
        };
        let name = name.trim();
        validate_project_env_name(name)
            .with_context(|| format!("line {} has an invalid variable name", index + 1))?;
        if value.contains('\0') {
            bail!("line {} contains a NUL byte", index + 1);
        }
        expand_env_vars(value).ok_or_else(|| {
            anyhow!(
                "line {} has invalid environment variable expansion syntax",
                index + 1
            )
        })?;
        env.insert(name.to_string(), value.to_string());
    }
    Ok(env)
}

fn validate_project_env_name(name: &str) -> Result<()> {
    if is_valid_var_name(name) {
        Ok(())
    } else {
        bail!("expected [A-Za-z_][A-Za-z0-9_]*")
    }
}

/// Expand environment variables (`$VAR`, `${VAR}`) and tilde (`~`) in a path
/// string.  Returns `None` when the result is unsafe (relative path, directory
/// traversal via `..`, or invalid variable names).
pub fn expand_path(raw: &str) -> Option<String> {
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    // Handle leading tilde.
    if chars.peek() == Some(&'~') {
        chars.next(); // consume '~'
        let home = home::home_dir()?;
        result.push_str(&home.to_string_lossy());
        // Allow `~/...` but also bare `~`.
        if chars.peek() == Some(&'/') {
            // keep the slash – the next iteration will push it
        } else if chars.peek().is_some() {
            // `~user` style – not supported, reject.
            return None;
        }
    }

    while let Some(ch) = chars.next() {
        if ch == '$' {
            // Try `${VAR}` or `$VAR`.
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next(); // consume '{'
            }
            let mut var_name = String::new();
            while let Some(&c) = chars.peek() {
                if braced {
                    if c == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                } else if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                var_name.push(c);
                chars.next();
            }
            // Validate variable name: [A-Za-z_][A-Za-z0-9_]*
            if var_name.is_empty() || !is_valid_var_name(&var_name) {
                return None;
            }
            match std::env::var(&var_name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    // Unresolved variable – keep the literal token so the user
                    // can see which variable failed in the warning message.
                    result.push('$');
                    if braced {
                        result.push('{');
                    }
                    result.push_str(&var_name);
                    if braced {
                        result.push('}');
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    let path = std::path::Path::new(&result);

    // Must be absolute.
    if !path.is_absolute() {
        return None;
    }

    // Reject directory traversal (`..`).
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }

    Some(result)
}

/// Returns `true` when `name` matches `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Top-level Config and KeysConfig
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Seconds the TUI waits for agents and companion terminals to exit after
    /// SIGTERM when quitting, before force-killing (SIGKILL) any stragglers.
    /// `0` skips the wait and force-kills immediately; values above
    /// [`MAX_SHUTDOWN_TIMEOUT_SECONDS`] are clamped (the unit is seconds, not
    /// milliseconds). A second Ctrl-c/SIGTERM during the wait cuts it short. A
    /// top-level (not `[ui]`/`[server]`) key because it is a global lifecycle
    /// knob. This governs the plain TUI quit; once the TUI is flipped into server
    /// mode, that shutdown uses `[server].shutdown_timeout_seconds` instead (even
    /// though you started in the TUI).
    pub shutdown_timeout_seconds: u16,
    pub defaults: Defaults,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub providers: ProvidersConfig,
    pub terminal: TerminalConfig,
    pub startup_command_terminal: StartupCommandTerminalConfig,
    pub logging: LoggingConfig,
    pub projects: Vec<ProjectConfig>,
    pub ui: UiConfig,
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
    pub editor: EditorConfig,
    #[serde(default)]
    pub server: ServerConfig,
    pub keys: KeysConfig,
    pub macros: MacrosConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub show_terminal_keys: bool,
    #[serde(flatten)]
    pub bindings: BTreeMap<String, Vec<String>>,
}

impl Default for KeysConfig {
    /// Returns a `KeysConfig` with no explicit bindings.
    ///
    /// The empty `bindings` map is intentional: default key assignments are
    /// resolved at runtime by `RuntimeBindings::from_keys_config` (in the TUI
    /// crate), which falls back to `BINDING_DEFS` for any action not present
    /// here. `dux-core` cannot reference `BINDING_DEFS` (it is `crokey`/
    /// `crossterm`-coupled, binary-only), so the defaults are deliberately
    /// omitted from this impl rather than duplicated.
    fn default() -> Self {
        Self {
            show_terminal_keys: true,
            bindings: BTreeMap::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shutdown_timeout_seconds: DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
            defaults: Defaults::default(),
            env: BTreeMap::new(),
            providers: ProvidersConfig::default(),
            terminal: TerminalConfig::default(),
            startup_command_terminal: StartupCommandTerminalConfig::default(),
            logging: LoggingConfig {
                level: "info".to_string(),
                path: "dux.log".to_string(),
            },
            projects: Vec::new(),
            ui: UiConfig {
                left_width_pct: 20,
                right_width_pct: 23,
                terminal_pane_height_pct: 35,
                empty_project_separator_min_projects: 5,
                staged_pane_height_pct: 50,
                commit_pane_height_pct: 40,
                agent_scrollback_lines: 10_000,
                agent_tabs_max: DEFAULT_AGENT_TABS_MAX,
                status_clear_seconds: 6,
                branch_sync_interval: 30,
                show_diff_line_numbers: false,
                diff_tab_width: 4,
                github_integration: true,
                pr_poll_interval_seconds: DEFAULT_PR_POLL_INTERVAL_SECONDS,
                copy_on_select: true,
                terminal_font_family: String::new(),
                terminal_font_size: DEFAULT_TERMINAL_FONT_SIZE,
                compose_bar: true,
                mobile_top_bar: true,
                mobile_accessory_bar: true,
                upload_directory: DEFAULT_UPLOAD_DIRECTORY.to_string(),
                upload_write_gitignore: true,
                attention_grace_seconds: 3,
                auto_reopen_agents: false,
                show_changes_pane: true,
                always_show_tab_strip: false,
                attention_indicator: true,
                attention_on_bell: true,
                disable_automated_welcome_screen: false,
                disable_release_notes: false,
                pr_banner_position: "bottom".to_string(),
                agent_sort: "active".to_string(),
                theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
            },
            capabilities: CapabilitiesConfig::default(),
            editor: EditorConfig::default(),
            server: ServerConfig::default(),
            keys: KeysConfig::default(),
            macros: MacrosConfig::default(),
        }
    }
}

impl Config {
    pub fn default_provider(&self) -> crate::model::ProviderKind {
        crate::model::ProviderKind::from_str(&self.defaults.provider)
    }
}

pub fn provider_config(
    config: &Config,
    provider: &crate::model::ProviderKind,
) -> ProviderCommandConfig {
    config
        .providers
        .get(provider.as_str())
        .cloned()
        .unwrap_or_else(|| ProviderCommandConfig {
            command: provider.as_str().to_string(),
            ..Default::default()
        })
}

/// Parse and validate `s` as a complete [`Config`] — the same `toml::from_str`
/// check [`load_config`] performs — returning the parsed value on success or a
/// user-facing error message on failure. The web's raw config editor calls this
/// to reject invalid TOML before it overwrites `config.toml`; it also uses the
/// returned value to compare security-sensitive sections against the running
/// config. Note the returned value is the raw parse (no provider defaults
/// applied); callers that want to *adopt* the config should reload from disk via
/// [`load_config`] so provider defaults are reapplied consistently.
pub fn validate_config_str(s: &str) -> Result<Config, String> {
    toml::from_str::<Config>(s).map_err(|e| e.to_string())
}

/// Load config for a read-only consumer (the web server). Reads `config.toml` if
/// present and parses it; on a missing file or parse error, falls back to defaults
/// (logging the error). Always applies provider defaults. Unlike the TUI's
/// `ensure_config`, this never creates, migrates, or writes the config file — the
/// server must not mutate config (that's the TUI's canonical renderer).
/// Deserialize `raw` into a [`Config`], recovering from a bad field or section
/// instead of discarding the entire file. A full-document parse success is used
/// directly; otherwise the offending key(s) are pruned — at FIELD granularity
/// where a single field can be isolated, else the whole top-level section — reset
/// to their defaults, warned to the log, and the rest is kept. A genuine TOML
/// syntax error (or a structure that can't be recovered) still falls back to
/// `Config::default()`. This means one bad value (e.g. `agent_tabs_max = -1`) can
/// never silently discard every other setting the user configured.
fn recover_config(raw: &str) -> Config {
    let doc: toml::Table = match toml::from_str::<toml::Table>(raw) {
        Ok(t) => t,
        Err(e) => {
            crate::logger::error(&format!("config is not valid TOML ({e}); using defaults"));
            return Config::default();
        }
    };
    // Fast path: the whole document deserializes cleanly.
    if let Ok(cfg) = table_into_config(doc.clone()) {
        return cfg;
    }
    // Recovery: a section that deserializes in isolation is fine (missing sections
    // use serde defaults, so a single-section document is always structurally ok).
    // For a bad section, drop only the offending FIELDS where each can be isolated;
    // otherwise reset the whole section.
    let mut pruned = doc.clone();
    for (section, value) in doc.iter() {
        if section_solo_ok(section, value.clone()) {
            continue;
        }
        if let toml::Value::Table(tbl) = value {
            let mut fixed = tbl.clone();
            let mut reset_fields: Vec<String> = Vec::new();
            for _ in 0..tbl.len() {
                if section_solo_ok(section, toml::Value::Table(fixed.clone())) {
                    break;
                }
                let field_keys: Vec<String> = fixed.keys().cloned().collect();
                let mut removed = false;
                for fk in field_keys {
                    let mut trial = fixed.clone();
                    trial.remove(&fk);
                    if section_solo_ok(section, toml::Value::Table(trial.clone())) {
                        reset_fields.push(fk);
                        fixed = trial;
                        removed = true;
                        break;
                    }
                }
                if !removed {
                    break;
                }
            }
            if section_solo_ok(section, toml::Value::Table(fixed.clone())) {
                for fk in &reset_fields {
                    crate::logger::warn(&format!(
                        "config [{section}] {fk} is invalid; resetting it to its default"
                    ));
                }
                pruned.insert(section.clone(), toml::Value::Table(fixed));
                continue;
            }
        }
        crate::logger::warn(&format!(
            "config section [{section}] is invalid; resetting it to defaults"
        ));
        pruned.remove(section);
    }
    match table_into_config(pruned) {
        Ok(cfg) => cfg,
        Err(e) => {
            crate::logger::error(&format!(
                "config could not be recovered ({e}); using defaults"
            ));
            Config::default()
        }
    }
}

/// True if a document containing only `section = value` deserializes into a
/// `Config` (every other section defaulted). Used to test one section at a time.
fn section_solo_ok(section: &str, value: toml::Value) -> bool {
    let mut t = toml::Table::new();
    t.insert(section.to_string(), value);
    table_into_config(t).is_ok()
}

/// Deserialize a `toml::Table` into a `Config`, round-tripping through a string so
/// this uses the exact same path as `toml::from_str` everywhere else (no reliance
/// on a `Value::try_into` API that varies across toml versions).
fn table_into_config(table: toml::Table) -> Result<Config, String> {
    let s = toml::to_string(&table).map_err(|e| e.to_string())?;
    toml::from_str::<Config>(&s).map_err(|e| e.to_string())
}

pub fn load_config(paths: &DuxPaths) -> Config {
    let mut config = match std::fs::read_to_string(&paths.config_path) {
        Ok(raw) => {
            // One-time migration notice: the single `[server] max_websocket_connections`
            // cap was split into three per-class caps. The unknown key is ignored on
            // load (ServerConfig has no deny_unknown_fields), so warn once so the
            // operator knows their old value is no longer in effect.
            warn_on_removed_max_websocket_connections(&raw);
            // Apply load-time config migrations IN MEMORY at every entrypoint
            // (deprecated `[server] bind` -> host/port, `prompt_for_name`, and
            // retired-provider pruning), so `dux serve` honors deprecated keys
            // instead of silently dropping them. The TUI's `ensure_config`
            // additionally PERSISTS the migrated document; here it is memory-only.
            // A parse or migration failure falls through to the normal recovery
            // path on the raw text (best effort, never fatal at load).
            let migrated = raw
                .parse::<toml_edit::DocumentMut>()
                .ok()
                .and_then(|mut doc| {
                    crate::config_migrate::apply_load_migrations(&mut doc)
                        .ok()
                        .map(|_| doc.to_string())
                })
                .unwrap_or_else(|| raw.clone());
            recover_config(&migrated)
        }
        Err(_) => Config::default(),
    };
    config.providers.ensure_defaults();
    // Surface a stale/unrecognized editor preference instead of silently falling
    // back to the first editor detected on PATH — e.g. a config left pointing at a
    // now-removed editor like "antigravity"/"windsurf".
    let configured_editor = config.editor.default.trim();
    if !configured_editor.is_empty() && crate::editor::editor_label(configured_editor).is_none() {
        crate::logger::warn(&format!(
            "config editor.default = \"{configured_editor}\" is not a recognized editor; \
             open-in-editor will fall back to the first one detected on PATH \
             (supported: cursor, vscode/code, zed, vscodium, sublime)"
        ));
    }
    // Surface an unrecognized clipboard_passthrough here, once at load, so the
    // per-tick forward path can parse without warning (see FIX-F5). from_config_str
    // logs the warning as a side effect and returns the fallback we discard.
    let _ = ClipboardPassthroughMode::from_config_str(&config.capabilities.clipboard_passthrough);
    // Same idea for a misspelled provider drag-and-drop paste form: warn ONCE here
    // rather than on every dropped file, and let the resolution itself stay silent.
    warn_on_unknown_web_dragdrop_paste_forms(&config.providers);
    // Same idea again for an out-of-range terminal_font_size: warn ONCE here and
    // correct it IN MEMORY, rather than letting `normalized_terminal_font_size`
    // (now pure, see its doc comment) warn on every bootstrap read. Correcting it
    // here also fixes on-disk persistence: a later save writes back the
    // already-valid in-memory value instead of re-persisting the bad one.
    if let Some(warning) = terminal_font_size_load_warning(config.ui.terminal_font_size) {
        crate::logger::warn(&warning);
        config.ui.terminal_font_size = DEFAULT_TERMINAL_FONT_SIZE;
    }
    // And once more for an upload directory that names somewhere dux will not
    // write (absolute, traversing, or empty). Corrected in memory here so every
    // later upload resolves the default silently rather than warning per file.
    if let Some(warning) = upload_directory_load_warning(&config.ui.upload_directory) {
        crate::logger::warn(&warning);
        config.ui.upload_directory = DEFAULT_UPLOAD_DIRECTORY.to_string();
    }
    config
}

/// The warning [`load_config`] emits when `ui.terminal_font_size` is outside
/// [`MIN_TERMINAL_FONT_SIZE`, `MAX_TERMINAL_FONT_SIZE`], or `None` for a value
/// already in range. Split out from the logging so the message can be asserted
/// directly, mirroring [`web_dragdrop_paste_warnings`].
fn terminal_font_size_load_warning(size: u16) -> Option<String> {
    if (MIN_TERMINAL_FONT_SIZE..=MAX_TERMINAL_FONT_SIZE).contains(&size) {
        return None;
    }
    Some(format!(
        "ui.terminal_font_size = {size} is outside the valid range \
         {MIN_TERMINAL_FONT_SIZE}..={MAX_TERMINAL_FONT_SIZE} and is being reset to \
         the default of {DEFAULT_TERMINAL_FONT_SIZE}."
    ))
}

/// Warn once per unrecognized `providers.<name>.web_dragdrop_paste` value, at load.
/// The value degrades to `bare` rather than failing the config load, exactly as an
/// unrecognized `capabilities.clipboard_passthrough` does; without this the
/// degradation would be silent and a user who typed `single-quoted` would never
/// learn why their dropped path stopped being quoted.
fn warn_on_unknown_web_dragdrop_paste_forms(providers: &ProvidersConfig) {
    for warning in web_dragdrop_paste_warnings(providers) {
        crate::logger::warn(&warning);
    }
}

/// Every warning a load would emit for an unrecognized
/// `providers.<name>.web_dragdrop_paste`, in config order: exactly one per
/// misspelled provider, and none at all for a config that is clean.
///
/// Split out from the logging so the messages can be asserted directly. The
/// per-paste resolution ([`ProviderCommandConfig::resolved_web_dragdrop_paste`])
/// deliberately does not go through this, which is what keeps the warning to once
/// per load rather than once per dropped file.
pub fn web_dragdrop_paste_warnings(providers: &ProvidersConfig) -> Vec<String> {
    providers
        .commands
        .iter()
        .filter_map(|(name, provider)| {
            WebDragDropPaste::unknown_value_warning(name, provider.web_dragdrop_paste.as_deref()?)
        })
        .collect()
}

/// Warn once when a `config.toml` still carries the removed
/// `[server] max_websocket_connections` key. Parses the raw TOML generically so a
/// commented-out line never trips the warning, then logs the three replacements
/// and the `=0` semantics change. The key itself is silently ignored on load (no
/// `deny_unknown_fields`), so this is the only place the operator learns their old
/// value stopped taking effect.
fn warn_on_removed_max_websocket_connections(raw: &str) {
    if raw_has_removed_max_websocket_connections(raw) {
        crate::logger::warn(
            "[server] max_websocket_connections has been removed and is being ignored. It \
             was split into max_websocket_events_connections, \
             max_websocket_agent_connections, and max_websocket_terminal_connections. Set \
             those per-class caps instead; a value of 0 still means disable (refuse all \
             new connections of that class until restart).",
        );
    }
}

/// Pure predicate behind the migration warning: true when the raw TOML has a
/// `[server] max_websocket_connections` key. Parses generically so a commented-out
/// line is not detected; a parse failure returns false (the loader surfaces the
/// real parse error separately).
///
/// `pub(crate)` so `config_write` can check the same condition on the write/strip
/// path (where the TUI saves config) and emit the warning there too.
pub(crate) fn raw_has_removed_max_websocket_connections(raw: &str) -> bool {
    toml::from_str::<toml::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("server")
                .and_then(toml::Value::as_table)
                .map(|server| server.contains_key("max_websocket_connections"))
        })
        .unwrap_or(false)
}

/// Check whether a provider command is available on PATH.
/// Returns `Ok(())` if found, or `Err(message)` with a user-friendly install hint.
pub fn check_provider_available(config: &ProviderCommandConfig) -> std::result::Result<(), String> {
    if provider_command_available(&config.command) {
        return Ok(());
    }

    let hint = config
        .install_hint
        .as_ref()
        .map(|h| format!("Install with: {h}"))
        .unwrap_or_else(|| {
            format!(
                "Make sure '{}' is installed and on your PATH.",
                config.command
            )
        });
    Err(format!(
        "CLI tool '{}' not found on PATH. {hint}",
        config.command
    ))
}

fn provider_command_available(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }

    let path = Path::new(command);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }

    env::var_os("PATH")
        .map(|paths| provider_command_available_in_path(command, &paths))
        .unwrap_or(false)
}

fn provider_command_available_in_path(command: &str, paths: &std::ffi::OsStr) -> bool {
    env::split_paths(paths).any(|dir| {
        let candidate = dir.join(command);
        is_executable_file(&candidate)
    })
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

/// One address in a [`ServerPlan`], tagged with whether binding it is REQUIRED or
/// merely BEST-EFFORT.
///
/// - `required: true` — a deliberate listener (the configured `host:port` or an
///   explicit `--bind`). A bind failure here is FATAL per the explicit-failure
///   tenet: the operator asked for this address, so refusing to serve it silently
///   would hide their intent.
/// - `required: false` — an opportunistic add-on. Today this is ONLY the
///   Tailscale leg of LOCAL MODE: it is auto-added when a Tailscale address is
///   detected, mirroring how tailscale-NOT-detected already degrades to loopback
///   with a warning. A bind failure here must NOT block the server — it warns
///   loudly and serves on the remaining (bound) addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanAddr {
    addr: std::net::SocketAddr,
    required: bool,
}

impl PlanAddr {
    /// A required (deliberate) listener — its bind failure is fatal.
    pub fn required(addr: std::net::SocketAddr) -> Self {
        Self {
            addr,
            required: true,
        }
    }

    /// A best-effort (opportunistic) listener — its bind failure degrades to a
    /// warning and the server continues on the remaining addresses.
    pub fn best_effort(addr: std::net::SocketAddr) -> Self {
        Self {
            addr,
            required: false,
        }
    }

    /// The socket address this listener targets.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Whether a bind failure on this address is fatal. `false` is only
    /// constructible via [`PlanAddr::best_effort`], so the best-effort invariant
    /// is enforced by the type rather than by call-site discipline.
    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// The fully-resolved listening plan for `dux server`, the single source of
/// truth the binary hands to dux-web. Lives in dux-core because both server entry
/// points and the resolver rules belong in the crate the binary and TUI share;
/// keeping the plan type here avoids a dux-web dependency from the resolver and
/// keeps the bind rules in one place.
///
/// The addresses are deduplicated and listed in a stable order, each tagged
/// [`PlanAddr::required`] (the configured/`--bind` primary, whose bind failure is
/// fatal) or [`PlanAddr::best_effort`] (the Tailscale leg, whose bind failure
/// degrades to a warning and serves the remaining addresses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerPlan {
    pub addrs: Vec<PlanAddr>,
}

/// CLI overrides for the server plan. Every field is `None`/`false` when the
/// operator passed nothing, so config values win by default and a present CLI
/// value takes precedence.
#[derive(Clone, Debug, Default)]
pub struct ServerCliOverrides {
    /// `--bind <ADDR:PORT>`: bind this exact address, overriding config host+port.
    pub bind: Option<String>,
    /// `--port <PORT>`: override `[server] port` only. Ignored when `bind` is set.
    pub port: Option<u16>,
    /// `--no-tailscale`: do not bind the Tailscale leg this run.
    pub no_tailscale: bool,
}

/// Resolve the complete `dux server` listening plan from config + CLI overrides +
/// the detected Tailscale address. This is the single source of truth for the
/// bind rules; the binary reads the returned [`ServerPlan`]'s addresses.
///
/// dux is trusted-local: the primary listener is always the configured
/// `host:port` (loopback by default) or an explicit `--bind`. There is no auth
/// gate and no public-bind refusal; the operator chooses the host directly and
/// the host guard (config `allowed_hosts`) governs which `Host` headers are
/// accepted. `tailscale_ip` is the detected Tailscale address (or `None` when
/// disabled / not detected); when present and not already covered by the primary
/// bind it is added as a BEST-EFFORT leg.
/// Parse a `[server] host` value into an [`std::net::IpAddr`], trimming
/// surrounding whitespace first. The SINGLE server-host parser, shared by
/// `resolve_server_plan` (the `dux server` bind path) and the TUI's early
/// `validate_server_host`, so a whitespaced host parses consistently at both
/// (previously the early check trimmed while the bind path did not, so
/// `" 0.0.0.0"` passed the check and then failed the actual bind). Hostnames are
/// not resolved; the value must be an IP literal such as `127.0.0.1` or
/// `0.0.0.0`.
pub fn parse_server_host(host: &str) -> Result<std::net::IpAddr, String> {
    use std::str::FromStr;
    let trimmed = host.trim();
    std::net::IpAddr::from_str(trimmed).map_err(|_| {
        format!(
            "[server] host = \"{trimmed}\" is not a valid IP address. Use an IP literal such as \
             127.0.0.1 (loopback) or 0.0.0.0 (all interfaces); hostnames are not resolved."
        )
    })
}

pub fn resolve_server_plan(
    server: &ServerConfig,
    cli: &ServerCliOverrides,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Result<ServerPlan> {
    let bind: std::net::SocketAddr = match cli.bind.as_deref() {
        Some(raw) => raw.parse().map_err(|_| {
            anyhow!(
                "invalid --bind address \"{raw}\": expected IP:port, e.g. 0.0.0.0:8080 \
                 (hostnames are not resolved)"
            )
        })?,
        None => {
            let host = parse_server_host(&server.host).map_err(|e| anyhow!("{e}"))?;
            std::net::SocketAddr::new(host, cli.port.unwrap_or(server.port))
        }
    };
    if bind.port() == 0 {
        bail!(
            "refusing to bind {bind}: port 0 means \"pick any free port\", so there would be no \
             stable address to open. Set [server] port (default 8080) or pass --port / --bind with \
             a non-zero port."
        );
    }
    let ts = if server.tailscale_enabled && !cli.no_tailscale {
        tailscale_ip
    } else {
        None
    };
    Ok(ServerPlan {
        addrs: plan_addrs(bind, ts),
    })
}

/// Primary address (REQUIRED) plus the Tailscale leg (BEST-EFFORT) when detected and
/// not already covered. A wildcard primary (0.0.0.0 / ::) already binds the Tailscale
/// interface, and an explicit bind to the Tailscale address is already in the list, so
/// both cases skip the extra leg.
pub(crate) fn plan_addrs(
    bind: std::net::SocketAddr,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Vec<PlanAddr> {
    let mut addrs = vec![PlanAddr::required(bind)];
    if let Some(ip) = tailscale_ip {
        let ts = std::net::SocketAddr::new(ip, bind.port());
        let subsumed = bind.ip().is_unspecified() || bind.ip() == ip;
        if !subsumed && !addrs.iter().any(|p| p.addr() == ts) {
            addrs.push(PlanAddr::best_effort(ts));
        }
    }
    addrs
}

/// LOCAL MODE bind addresses for the TUI palette flip: loopback (REQUIRED) plus the
/// Tailscale leg. A thin wrapper over `plan_addrs` so the flip can never open a
/// non-loopback primary listener.
pub fn local_addrs(port: u16, tailscale_ip: Option<std::net::IpAddr>) -> Vec<PlanAddr> {
    plan_addrs(
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        tailscale_ip,
    )
}

#[cfg(test)]
mod local_addrs_tests {
    use super::{PlanAddr, local_addrs};

    #[test]
    fn loopback_only_when_no_tailscale() {
        let addrs = local_addrs(8080, None);
        assert_eq!(
            addrs,
            vec![PlanAddr::required("127.0.0.1:8080".parse().unwrap())]
        );
    }

    #[test]
    fn loopback_required_tailscale_best_effort_when_present() {
        // Loopback is REQUIRED (a bind failure is fatal); the auto-added Tailscale
        // leg is BEST-EFFORT (a bind failure degrades to loopback + a warning).
        let ts = "100.101.102.103".parse().unwrap();
        let addrs = local_addrs(9090, Some(ts));
        assert_eq!(
            addrs,
            vec![
                PlanAddr::required("127.0.0.1:9090".parse().unwrap()),
                PlanAddr::best_effort("100.101.102.103:9090".parse().unwrap()),
            ]
        );
        assert!(addrs[0].is_required(), "loopback must be required");
        assert!(
            !addrs[1].is_required(),
            "the Tailscale leg must be best-effort"
        );
    }

    #[test]
    fn tailscale_ipv6_uses_bracketed_socketaddr() {
        let ts = "fd7a:115c:a1e0::1".parse().unwrap();
        let addrs = local_addrs(8080, Some(ts));
        assert_eq!(
            addrs,
            vec![
                PlanAddr::required("127.0.0.1:8080".parse().unwrap()),
                PlanAddr::best_effort("[fd7a:115c:a1e0::1]:8080".parse().unwrap()),
            ]
        );
    }
}

#[cfg(test)]
mod resolve_plan_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    fn ts() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(100, 100, 0, 1))
    }
    fn cli() -> ServerCliOverrides {
        ServerCliOverrides::default()
    }

    #[test]
    fn default_loopback_only_without_tailscale() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:8080".parse().unwrap())]
        );
    }
    #[test]
    fn default_adds_best_effort_tailscale_leg() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), Some(ts())).unwrap();
        assert_eq!(p.addrs.len(), 2);
        assert!(!p.addrs[1].is_required());
    }
    #[test]
    fn no_tailscale_suppresses_leg() {
        let c = ServerCliOverrides {
            no_tailscale: true,
            ..cli()
        };
        assert_eq!(
            resolve_server_plan(&ServerConfig::default(), &c, Some(ts()))
                .unwrap()
                .addrs
                .len(),
            1
        );
    }
    #[test]
    fn bind_wildcard_overrides_and_subsumes_tailscale() {
        let c = ServerCliOverrides {
            bind: Some("0.0.0.0:9000".into()),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, Some(ts())).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("0.0.0.0:9000".parse().unwrap())]
        );
    }
    #[test]
    fn port_flag_overrides_only_port() {
        let c = ServerCliOverrides {
            port: Some(7000),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:7000".parse().unwrap())]
        );
    }
    #[test]
    fn bind_beats_port() {
        let c = ServerCliOverrides {
            bind: Some("127.0.0.1:1234".into()),
            port: Some(7000),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:1234".parse().unwrap())]
        );
    }
    #[test]
    fn port_zero_refused() {
        let c = ServerConfig {
            port: 0,
            ..ServerConfig::default()
        };
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
    #[test]
    fn invalid_bind_refused() {
        let c = ServerCliOverrides {
            bind: Some("nope".into()),
            ..cli()
        };
        assert!(resolve_server_plan(&ServerConfig::default(), &c, None).is_err());
    }
    #[test]
    fn invalid_host_refused() {
        let c = ServerConfig {
            host: "example.com".into(),
            ..ServerConfig::default()
        };
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn shutdown_grace_converts_and_clamps_to_the_ceiling() {
        use std::time::Duration;
        // 0 = immediate force; ordinary values pass through as seconds.
        assert_eq!(shutdown_grace(0), Duration::ZERO);
        assert_eq!(shutdown_grace(30), Duration::from_secs(30));
        // At and below the ceiling, unchanged.
        assert_eq!(
            shutdown_grace(MAX_SHUTDOWN_TIMEOUT_SECONDS),
            Duration::from_secs(u64::from(MAX_SHUTDOWN_TIMEOUT_SECONDS))
        );
        // A fat-fingered millisecond value (e.g. 30000) is clamped to the ceiling
        // rather than blocking shutdown for hours.
        assert_eq!(
            shutdown_grace(30_000),
            Duration::from_secs(u64::from(MAX_SHUTDOWN_TIMEOUT_SECONDS))
        );
        assert_eq!(
            shutdown_grace(u16::MAX),
            Duration::from_secs(u64::from(MAX_SHUTDOWN_TIMEOUT_SECONDS))
        );
    }

    #[test]
    fn ui_config_default_terminal_font_settings() {
        let ui = UiConfig::default();
        assert_eq!(ui.terminal_font_family, "");
        assert_eq!(ui.terminal_font_size, DEFAULT_TERMINAL_FONT_SIZE);
    }

    #[test]
    fn normalized_terminal_font_size_passes_through_in_range_values() {
        assert_eq!(
            normalized_terminal_font_size(MIN_TERMINAL_FONT_SIZE),
            MIN_TERMINAL_FONT_SIZE
        );
        assert_eq!(
            normalized_terminal_font_size(MAX_TERMINAL_FONT_SIZE),
            MAX_TERMINAL_FONT_SIZE
        );
        assert_eq!(normalized_terminal_font_size(18), 18);
    }

    #[test]
    fn normalized_terminal_font_size_degrades_out_of_range_values_to_the_default() {
        assert_eq!(
            normalized_terminal_font_size(MIN_TERMINAL_FONT_SIZE - 1),
            DEFAULT_TERMINAL_FONT_SIZE
        );
        assert_eq!(
            normalized_terminal_font_size(MAX_TERMINAL_FONT_SIZE + 1),
            DEFAULT_TERMINAL_FONT_SIZE
        );
        assert_eq!(normalized_terminal_font_size(0), DEFAULT_TERMINAL_FONT_SIZE);
        assert_eq!(
            normalized_terminal_font_size(u16::MAX),
            DEFAULT_TERMINAL_FONT_SIZE
        );
    }

    #[test]
    fn terminal_font_size_load_warning_is_none_for_in_range_values() {
        assert_eq!(
            terminal_font_size_load_warning(MIN_TERMINAL_FONT_SIZE),
            None
        );
        assert_eq!(
            terminal_font_size_load_warning(MAX_TERMINAL_FONT_SIZE),
            None
        );
        assert_eq!(terminal_font_size_load_warning(18), None);
    }

    #[test]
    fn terminal_font_size_load_warning_names_the_bad_value_and_the_default() {
        let warning = terminal_font_size_load_warning(500).expect("warning expected");
        assert!(warning.contains("500"));
        assert!(warning.contains(&DEFAULT_TERMINAL_FONT_SIZE.to_string()));
    }

    #[test]
    fn load_config_degrades_an_out_of_range_terminal_font_size_and_persists_the_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(&paths.config_path, "[ui]\nterminal_font_size = 500\n")
            .expect("write config");

        let config = load_config(&paths);
        // The in-memory value is corrected at load, so `normalized_terminal_font_size`
        // (now a pure clamp used elsewhere) never has to run to reach a valid value,
        // and a later save persists the corrected default rather than 500.
        assert_eq!(config.ui.terminal_font_size, DEFAULT_TERMINAL_FONT_SIZE);
    }

    #[test]
    fn load_config_leaves_an_in_range_terminal_font_size_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(&paths.config_path, "[ui]\nterminal_font_size = 22\n")
            .expect("write config");

        let config = load_config(&paths);
        assert_eq!(config.ui.terminal_font_size, 22);
    }

    // ── ui.upload_directory ──────────────────────────────────────────────────

    #[test]
    fn a_usable_upload_directory_is_kept_as_written() {
        for value in [".dux/uploads", "uploads", "tmp/dux/drops", ".uploads"] {
            assert_eq!(normalized_upload_directory(value), value);
            assert_eq!(upload_directory_load_warning(value), None, "for {value:?}");
        }
    }

    #[test]
    fn an_upload_directory_is_normalized_to_its_components() {
        // Surrounding whitespace and repeated or trailing separators are
        // cosmetic, not a rejection: the walk that creates the directory works
        // in components anyway, so these all name the same place.
        assert_eq!(normalized_upload_directory("  uploads  "), "uploads");
        assert_eq!(normalized_upload_directory("uploads/"), "uploads");
        assert_eq!(normalized_upload_directory(".dux//uploads"), ".dux/uploads");
    }

    #[test]
    fn a_curdir_component_is_normalized_away_wherever_it_sits() {
        // `./uploads` is an ordinary way to write a relative path and names
        // exactly the same directory as `uploads`, so rejecting it bought no
        // safety at all. It was also INCONSISTENT, which is how the defect
        // showed: MEASURED, `Path::components()` keeps a CurDir only in LEADING
        // position and drops it everywhere else, so `uploads/./x` was already
        // being accepted and quietly normalized to `uploads/x` while the
        // leading form was refused.
        assert_eq!(normalized_upload_directory("./uploads"), "uploads");
        assert_eq!(normalized_upload_directory("./"), DEFAULT_UPLOAD_DIRECTORY);
        assert_eq!(upload_directory_load_warning("./uploads"), None);
        assert_eq!(normalized_upload_directory("uploads/./x"), "uploads/x");
        assert_eq!(upload_directory_load_warning("uploads/./x"), None);
        // A trailing `.` is dropped by `components()` too, leaving the named
        // directory behind, so it names a real place and is kept.
        assert_eq!(normalized_upload_directory("uploads/."), "uploads");
        assert_eq!(upload_directory_load_warning("uploads/."), None);
    }

    #[test]
    fn an_unusable_upload_directory_degrades_to_the_default_and_says_why() {
        // Each of these would put a dropped file somewhere the agent's worktree
        // does not own, so each degrades rather than being obeyed. The check is
        // on the path SHAPE; a symlinked component is refused later, at
        // creation time, by `file_drop::DropDir::open_uploads`.
        //
        // The last three are a different kind of unusable: they name a shape
        // the filesystem itself will refuse. They are checked HERE, at load,
        // because the alternative is what shipped: the value passes, the
        // directory is even created for the control-character case, and then
        // every single drop fails at the syscall with a message about the wrong
        // thing entirely (`Invalid argument` for a NUL, `File name too long`
        // for an over-long one). Catching them at load is the whole point of
        // the warn-once-and-degrade design.
        let long_component = "a".repeat(crate::file_drop::FALLBACK_NAME_MAX_BYTES + 1);
        let long_path = "ab/".repeat(MAX_UPLOAD_DIRECTORY_BYTES / 3 + 1);
        let cases = [
            ("", "empty"),
            ("   ", "empty"),
            ("/tmp/uploads", "absolute"),
            ("/", "absolute"),
            ("../uploads", ".."),
            (".dux/../../uploads", ".."),
            ("uploads/..", ".."),
            (".", "names no directory"),
            ("./", "names no directory"),
            ("uploads\nx", "control character"),
            ("uploads\u{1b}x", "control character"),
            ("uploads\u{0}x", "null byte"),
            (long_component.as_str(), "component"),
            (long_path.as_str(), "longer than"),
        ];
        for (value, expected_reason) in cases {
            assert_eq!(
                normalized_upload_directory(value),
                DEFAULT_UPLOAD_DIRECTORY,
                "{value:?} must degrade to the default"
            );
            let warning = upload_directory_load_warning(value)
                .unwrap_or_else(|| panic!("{value:?} must warn"));
            assert!(
                warning.contains(expected_reason),
                "the warning for {value:?} must say why ({expected_reason}), got: {warning}"
            );
            assert!(
                warning.contains(DEFAULT_UPLOAD_DIRECTORY),
                "the warning for {value:?} must name the fallback, got: {warning}"
            );
        }
    }

    #[test]
    fn load_config_corrects_a_bad_upload_directory_so_nothing_can_warn_about_it_again() {
        // What this pins is the CORRECTION, which is the thing that makes the
        // warn-once property true: `load_config` replaces the value in memory,
        // so every later read (and every save) sees a usable directory and has
        // nothing left to warn about. Without it the pure normalizer would
        // silently degrade on every single upload instead.
        //
        // It deliberately does NOT observe the log line, and the name no longer
        // claims to. `logger::warn` writes through a process-wide `OnceLock`
        // that any other test may already have set, so watching it from here
        // would be order-dependent. The line itself is asserted directly, from
        // the pure `upload_directory_load_warning`, by
        // `an_unusable_upload_directory_degrades_to_the_default_and_says_why`.
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(
            &paths.config_path,
            "[ui]\nupload_directory = \"../escape\"\n",
        )
        .expect("write config");

        assert!(
            upload_directory_load_warning("../escape").is_some(),
            "the value on disk must be one that warns, or this proves nothing"
        );
        let config = load_config(&paths);
        assert_eq!(config.ui.upload_directory, DEFAULT_UPLOAD_DIRECTORY);
        assert_eq!(
            upload_directory_load_warning(&config.ui.upload_directory),
            None,
            "the corrected value must not warn a second time"
        );
    }

    #[test]
    fn load_config_leaves_a_usable_upload_directory_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(
            &paths.config_path,
            "[ui]\nupload_directory = \"tmp/drops\"\nupload_write_gitignore = false\n",
        )
        .expect("write config");

        let config = load_config(&paths);
        assert_eq!(config.ui.upload_directory, "tmp/drops");
        assert!(!config.ui.upload_write_gitignore);
    }

    #[test]
    fn validate_config_str_accepts_a_full_render_and_rejects_garbage() {
        // The canonical plain render of the default config must round-trip
        // through the validator (this is exactly what the web editor writes).
        let rendered = crate::config_write::render_config_plain(&Config::default());
        assert!(
            validate_config_str(&rendered).is_ok(),
            "rendered default config must validate:\n{rendered}"
        );
        assert!(
            validate_config_str("this is = = not valid toml").is_err(),
            "garbage must be rejected"
        );
        // Structurally-valid TOML with a wrong-typed field must also be rejected
        // (deserialization failure, not just a parse failure) — otherwise the web
        // editor would accept a value the engine can't load.
        assert!(
            validate_config_str("[ui]\nagent_scrollback_lines = \"lots\"\n").is_err(),
            "a string for a numeric field must be rejected"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn config_root_uses_hidden_home_dir_on_macos() {
        let root = discover_root(Path::new("/example/home"), Some("/tmp/ignored".into()));
        assert_eq!(root, PathBuf::from("/example/home/.dux"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_uses_xdg_config_home_when_absolute() {
        let root = discover_root(Path::new("/example/home"), Some("/tmp/xdg".into()));
        assert_eq!(root, PathBuf::from("/tmp/xdg/dux"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_falls_back_to_dot_config_when_xdg_missing() {
        let root = discover_root(Path::new("/example/home"), None);
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_ignores_relative_xdg_config_home() {
        let root = discover_root(Path::new("/example/home"), Some("relative/path".into()));
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    #[test]
    fn resolve_root_uses_dux_home_when_absolute() {
        let root = resolve_root(Some("/custom/dux".into()), None, None).unwrap();
        assert_eq!(root, PathBuf::from("/custom/dux"));
    }

    #[test]
    fn resolve_root_errors_on_relative_dux_home() {
        let err = resolve_root(
            Some("relative/path".into()),
            Some(PathBuf::from("/home")),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("DUX_HOME must be an absolute path"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("relative/path"),
            "error should contain the bad path: {err}"
        );
    }

    /// The config directory's mode is what actually protects the files inside
    /// it, because SQLite creates its `-wal`/`-shm` sidecars itself at runtime
    /// and offers no way to set their mode. A directory another local user
    /// cannot search makes the files' own modes moot.
    #[test]
    fn ensure_dirs_makes_the_config_root_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        let paths = test_paths(&root);
        paths.ensure_dirs().unwrap();
        let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected 0700, got {mode:o}");
    }

    /// Everyone upgrading already has a `0755` directory, so tightening has to
    /// happen on startup or the change reaches nobody. `ensure_dirs` runs on
    /// every startup and the tightening is idempotent.
    #[test]
    fn ensure_dirs_tightens_a_config_root_left_world_readable_by_an_older_install() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        let paths = test_paths(&root);
        paths.ensure_dirs().unwrap();
        let mode = fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected 0700, got {mode:o}");
    }

    /// The config file is what the shipped template comment promises is made
    /// `0600` on EVERY start, and it was the one file nothing tightened.
    /// `write_config_atomic` creates its temp at `0600` and renames over the
    /// original, which is why a fresh install looks correct, but that only runs
    /// on first creation or when a save actually changes the document, so a
    /// `config.toml` chmod'd to `0644` by hand stayed `0644` forever while the
    /// log and the database were corrected on every open. It holds `[env]`
    /// tokens, so it is the file the promise mattered most for.
    #[test]
    fn ensure_dirs_tightens_a_config_file_left_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        fs::create_dir_all(&root).unwrap();
        let paths = test_paths(&root);
        fs::write(&paths.config_path, "[env]\nTOKEN = \"secret\"\n").unwrap();
        fs::set_permissions(&paths.config_path, fs::Permissions::from_mode(0o644)).unwrap();

        paths.ensure_dirs().unwrap();

        let mode = fs::metadata(&paths.config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    /// And the other half of the same promise: dux only ever REMOVES group and
    /// world access, so a config deliberately left read-only at `0400` keeps
    /// its owner bits through the tightening pass.
    #[test]
    fn ensure_dirs_leaves_a_read_only_config_read_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        fs::create_dir_all(&root).unwrap();
        let paths = test_paths(&root);
        fs::write(&paths.config_path, "[env]\n").unwrap();
        fs::set_permissions(&paths.config_path, fs::Permissions::from_mode(0o400)).unwrap();

        paths.ensure_dirs().unwrap();

        let mode = fs::metadata(&paths.config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o400, "expected 0400, got {mode:o}");
    }

    /// A config that is not there yet must not be an error, and must not be
    /// conjured into existence by the tightening pass.
    #[test]
    fn ensure_dirs_does_not_create_a_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        let paths = test_paths(&root);
        paths.ensure_dirs().unwrap();
        assert!(!paths.config_path.exists());
    }

    /// The worktrees directory holds the user's own checkouts, which they open
    /// in their own editor. dux does not tighten it; it sits inside the
    /// now-`0700` root, which is where the protection belongs.
    ///
    /// The GROUP AND OTHER bits are what this test is actually about. It used
    /// to assert `mode & 0o700 == 0o700`, which is equally true of `0755` and
    /// `0700`, so tightening the worktrees directory left it passing and the
    /// tenet it exists to defend was unprotected. The directory is pre-created
    /// at a known `0755` so the assertion does not depend on the test runner's
    /// umask.
    #[test]
    fn ensure_dirs_leaves_the_worktrees_directory_untightened() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        let paths = test_paths(&root);
        fs::create_dir_all(&paths.worktrees_root).unwrap();
        fs::set_permissions(&paths.worktrees_root, fs::Permissions::from_mode(0o755)).unwrap();

        paths.ensure_dirs().unwrap();

        let mode = fs::metadata(&paths.worktrees_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "the worktrees directory must be left exactly as it was, got {mode:o}"
        );
        assert_ne!(
            mode & 0o077,
            0,
            "the group and other bits must survive; dux does not tighten the user's checkouts"
        );
    }

    /// The companion to the above: a freshly created worktrees directory is not
    /// tightened either. Asserted against the process umask so it stays true
    /// wherever it runs, and it still fails outright if dux starts forcing
    /// `0700` here.
    #[test]
    fn ensure_dirs_creates_the_worktrees_directory_without_tightening_it() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("dux");
        let paths = test_paths(&root);
        paths.ensure_dirs().unwrap();

        // What the umask alone would leave on a new directory, measured by
        // making one right here rather than assumed.
        let reference = tmp.path().join("reference");
        fs::create_dir(&reference).unwrap();
        let expected = fs::metadata(&reference).unwrap().permissions().mode() & 0o777;

        let mode = fs::metadata(&paths.worktrees_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, expected,
            "expected the umask default {expected:o}, got {mode:o}"
        );
    }

    fn test_paths(root: &std::path::Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    #[test]
    fn resolve_root_errors_on_empty_dux_home() {
        let err = resolve_root(Some("".into()), Some(PathBuf::from("/home")), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("DUX_HOME must be an absolute path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_root_falls_through_when_dux_home_unset() {
        let root = resolve_root(None, Some(PathBuf::from("/example/home")), None).unwrap();
        // Should delegate to discover_root with platform defaults
        #[cfg(target_os = "macos")]
        assert_eq!(root, PathBuf::from("/example/home/.dux"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    // ── expand_path tests ────────────────────────────────────────────────

    #[test]
    fn expand_path_absolute_unchanged() {
        assert_eq!(
            expand_path("/absolute/path").as_deref(),
            Some("/absolute/path")
        );
    }

    #[test]
    fn expand_path_tilde() {
        let home = home::home_dir().expect("home dir");
        let result = expand_path("~/projects/foo").unwrap();
        assert_eq!(result, format!("{}/projects/foo", home.display()));
    }

    #[test]
    fn expand_path_bare_tilde() {
        let home = home::home_dir().expect("home dir");
        assert_eq!(expand_path("~").unwrap(), home.to_string_lossy());
    }

    #[test]
    fn expand_path_dollar_var() {
        // SAFETY: test-only env manipulation; tests are run with --test-threads=1
        // or use unique variable names to avoid races.
        unsafe { std::env::set_var("DUX_TEST_VAR_1", "/test/value") };
        let result = expand_path("$DUX_TEST_VAR_1/subdir").unwrap();
        assert_eq!(result, "/test/value/subdir");
        unsafe { std::env::remove_var("DUX_TEST_VAR_1") };
    }

    #[test]
    fn expand_path_braced_var() {
        unsafe { std::env::set_var("DUX_TEST_VAR_2", "/braced") };
        let result = expand_path("${DUX_TEST_VAR_2}/sub").unwrap();
        assert_eq!(result, "/braced/sub");
        unsafe { std::env::remove_var("DUX_TEST_VAR_2") };
    }

    #[test]
    fn expand_path_unresolved_var_kept_literal() {
        // Unresolved var is preserved literally; if the overall path is still
        // absolute the function succeeds (the path just won't exist on disk).
        let result = expand_path("/prefix/$NONEXISTENT_DUX_VAR_999/suffix");
        assert_eq!(
            result.as_deref(),
            Some("/prefix/$NONEXISTENT_DUX_VAR_999/suffix")
        );
    }

    #[test]
    fn expand_path_rejects_relative() {
        assert!(expand_path("relative/path").is_none());
    }

    #[test]
    fn expand_path_rejects_dotdot_relative() {
        assert!(expand_path("../relative/path").is_none());
    }

    #[test]
    fn expand_path_rejects_traversal() {
        unsafe { std::env::set_var("DUX_TEST_VAR_3", "/safe") };
        assert!(expand_path("$DUX_TEST_VAR_3/../etc/passwd").is_none());
        unsafe { std::env::remove_var("DUX_TEST_VAR_3") };
    }

    #[test]
    fn expand_path_rejects_command_substitution() {
        assert!(expand_path("$(whoami)/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_tilde_user() {
        // `~otheruser/foo` is not supported.
        assert!(expand_path("~otheruser/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_empty_var_name() {
        assert!(expand_path("$/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_empty_braced_var_name() {
        assert!(expand_path("${}/foo").is_none());
    }

    #[test]
    fn project_env_lines_parse_and_expand() {
        unsafe { std::env::set_var("DUX_TEST_PROJECT_ENV_SOURCE", "secret") };
        let env = parse_project_env_lines("EDITOR=true\nAPI_KEY=${DUX_TEST_PROJECT_ENV_SOURCE}")
            .expect("parse env");
        let resolved = resolve_project_env(&env).expect("resolve env");
        assert!(resolved.contains(&("EDITOR".to_string(), "true".to_string())));
        assert!(resolved.contains(&("API_KEY".to_string(), "secret".to_string())));
        unsafe { std::env::remove_var("DUX_TEST_PROJECT_ENV_SOURCE") };
    }

    #[test]
    fn project_env_lines_reject_invalid_names_and_expansions() {
        assert!(parse_project_env_lines("1BAD=value").is_err());
        assert!(parse_project_env_lines("GOOD=${}").is_err());
        assert!(parse_project_env_lines("MISSING_EQUALS").is_err());
    }

    #[test]
    fn agent_env_merges_global_and_project_with_project_override() {
        let global = BTreeMap::from([
            ("EDITOR".to_string(), "true".to_string()),
            ("API_KEY".to_string(), "global".to_string()),
        ]);
        let project = BTreeMap::from([("API_KEY".to_string(), "project".to_string())]);

        let resolved = resolve_agent_env(&global, &project).expect("resolve env");

        assert!(resolved.contains(&("EDITOR".to_string(), "true".to_string())));
        assert!(resolved.contains(&("API_KEY".to_string(), "project".to_string())));
        assert!(!resolved.contains(&("API_KEY".to_string(), "global".to_string())));
    }

    fn write_executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\nexit 0\n").expect("write fixture command");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .expect("fixture metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod fixture command");
    }

    #[test]
    fn provider_command_path_lookup_accepts_executable_from_path() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        write_executable(&command);
        let paths = std::env::join_paths([dir.path()]).expect("join path");

        assert!(provider_command_available_in_path(
            "custom-tool",
            paths.as_os_str()
        ));
    }

    #[test]
    fn provider_command_path_lookup_rejects_missing_command() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let paths = std::env::join_paths([dir.path()]).expect("join path");

        assert!(!provider_command_available_in_path(
            "missing-tool",
            paths.as_os_str()
        ));
    }

    #[test]
    fn provider_command_path_lookup_accepts_absolute_executable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        write_executable(&command);

        assert!(provider_command_available(&command.to_string_lossy()));
    }

    #[test]
    fn provider_command_path_lookup_rejects_non_executable_path() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        std::fs::write(&command, "#!/bin/sh\n").expect("write fixture command");

        assert!(!provider_command_available(&command.to_string_lossy()));
    }

    #[test]
    fn provider_availability_error_uses_install_hint() {
        let cfg = ProviderCommandConfig {
            command: "definitely-missing-provider-command".to_string(),
            install_hint: Some("install custom-tool".to_string()),
            ..Default::default()
        };

        let err = check_provider_available(&cfg).expect_err("command should be missing");
        assert!(err.contains("definitely-missing-provider-command"));
        assert!(err.contains("Install with: install custom-tool"));
    }

    // ── load_config tests ────────────────────────────────────────────────

    fn make_test_paths(root: &std::path::Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    /// #30: the ONE server-host parser is trimming and shared, so a config with
    /// surrounding whitespace parses consistently at the early check and at
    /// `resolve_server_plan` (which previously parsed untrimmed and failed a
    /// value the TUI's trimming check had accepted).
    #[test]
    fn parse_server_host_trims_and_parses() {
        assert_eq!(
            parse_server_host("  0.0.0.0  ").expect("ok"),
            std::net::IpAddr::from([0, 0, 0, 0])
        );
        assert!(parse_server_host("not-an-ip").is_err());
    }

    #[test]
    fn resolve_server_plan_accepts_a_whitespaced_host() {
        let server = ServerConfig {
            host: " 0.0.0.0 ".to_string(),
            ..ServerConfig::default()
        };
        let plan = resolve_server_plan(&server, &ServerCliOverrides::default(), None)
            .expect("a whitespaced host must resolve");
        assert!(
            plan.addrs
                .iter()
                .any(|a| a.addr().ip() == std::net::IpAddr::from([0, 0, 0, 0]))
        );
    }

    /// #4: a deprecated `[server] bind` with a NON-loopback address must be
    /// migrated IN MEMORY by `load_config` (host + port), so every entrypoint,
    /// including `dux serve`, honors it instead of silently binding loopback.
    #[test]
    fn load_config_migrates_deprecated_server_bind_in_memory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(&paths.config_path, "[server]\nbind = \"0.0.0.0:9000\"\n")
            .expect("write config");

        let config = load_config(&paths);
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
    }

    /// #31: an untouched stock block for a retired provider (gemini) must be
    /// pruned IN MEMORY by `load_config` so `dux serve`'s provider pickers stop
    /// offering it (previously only the TUI's `ensure_config` pruned it).
    #[test]
    fn load_config_prunes_a_retired_stock_provider_in_memory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        // The exact stock gemini block dux shipped (as its renderer wrote it).
        std::fs::write(
            &paths.config_path,
            "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\ninstall_hint = \"brew install gemini-cli\"\n",
        )
        .expect("write config");

        let config = load_config(&paths);
        assert!(
            !config.providers.commands.contains_key("gemini"),
            "an untouched stock retired provider must be pruned"
        );
    }

    /// #31: a CUSTOMIZED block for a retired provider is preserved (config wins
    /// for explicit preferences).
    #[test]
    fn load_config_keeps_a_customized_retired_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(
            &paths.config_path,
            "[providers.gemini]\ncommand = \"/opt/my-gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\n",
        )
        .expect("write config");

        let config = load_config(&paths);
        assert!(
            config.providers.commands.contains_key("gemini"),
            "a user-customized retired provider block must be kept"
        );
    }

    #[test]
    fn load_config_reads_custom_provider_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(
            &paths.config_path,
            r#"
[providers.claude]
command = "/custom/claude"

[ui]
github_integration = false
"#,
        )
        .expect("write config");

        let config = load_config(&paths);

        assert_eq!(
            config.providers.commands["claude"].command, "/custom/claude",
            "custom provider command should be loaded from config.toml"
        );
        assert!(
            !config.ui.github_integration,
            "ui.github_integration should be false per config.toml"
        );
        // Provider defaults must still be populated (e.g. codex should exist).
        assert!(
            config.providers.commands.contains_key("codex"),
            "ensure_defaults should add missing default providers"
        );
    }

    #[test]
    fn load_config_falls_back_to_defaults_when_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        // No config.toml written — file does not exist.

        let config = load_config(&paths);

        // Provider defaults must be present.
        assert!(
            config.providers.commands.contains_key("claude"),
            "claude provider should be present via defaults"
        );
        assert!(
            config.providers.commands.contains_key("codex"),
            "codex provider should be present via defaults"
        );
    }

    #[test]
    fn server_config_defaults_when_section_absent() {
        // A config TOML with no [server] section must still parse and yield the
        // safe local defaults (loopback host, port 8080, Tailscale opt-out on).
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.allowed_hosts.is_empty());
    }

    #[test]
    fn server_title_defaults_to_dux_and_parses_override() {
        // No [server] section: title defaults to the product name.
        let default: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(default.server.title, "dux");

        // An explicit title (e.g. to tell multiple instances apart) round-trips.
        let config: Config = toml::from_str(
            r#"
[server]
title = "dux #1"
"#,
        )
        .expect("config with [server] title should parse");
        assert_eq!(config.server.title, "dux #1");
    }

    #[test]
    fn server_favicon_defaults_empty_and_parses_override() {
        // No [server] section: favicon is empty, meaning "use the bundled logo".
        let default: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(default.server.favicon, "");

        // An explicit favicon (a colour, here) round-trips verbatim; the web
        // interprets the string (colour vs URL vs default).
        let config: Config = toml::from_str(
            r#"
[server]
favicon = "violet"
"#,
        )
        .expect("config with [server] favicon should parse");
        assert_eq!(config.server.favicon, "violet");
    }

    #[test]
    fn server_config_parses_full_section() {
        let config: Config = toml::from_str(
            r#"
[server]
host = "0.0.0.0"
port = 9000
tailscale_enabled = false
allowed_hosts = ["box.tailnet.ts.net"]
"#,
        )
        .expect("config with full [server] should parse");
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        assert!(!config.server.tailscale_enabled);
        assert_eq!(
            config.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
    }

    #[test]
    fn server_config_partial_section_defaults_remaining_fields() {
        // Only `port` is provided; the rest default.
        let config: Config = toml::from_str(
            r#"
[server]
port = 9000
"#,
        )
        .expect("config with partial [server] should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9000);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.allowed_hosts.is_empty());
    }

    /// Deserializing a `[server]` table that omits all three `max_websocket_*` keys
    /// must yield the expected defaults via the container `#[serde(default)]` plus
    /// the manual `Default` impl. Pinned so a serde refactor cannot silently zero
    /// out the caps.
    #[test]
    fn server_config_websocket_caps_default_when_keys_absent() {
        let config: Config = toml::from_str(
            r#"
[server]
port = 8080
"#,
        )
        .expect("config without max_websocket_* keys should parse");
        assert_eq!(
            config.server.max_websocket_events_connections,
            DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
            "events cap must default to {DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS}"
        );
        assert_eq!(
            config.server.max_websocket_agent_connections, DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
            "agent cap must default to {DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS}"
        );
        assert_eq!(
            config.server.max_websocket_terminal_connections,
            DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
            "terminal cap must default to {DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS}"
        );
    }

    #[test]
    fn load_config_falls_back_to_defaults_on_malformed_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(&paths.config_path, "this is not valid toml ][[[")
            .expect("write bad config");

        // Must not panic; must return usable defaults.
        let config = load_config(&paths);

        assert!(
            config.providers.commands.contains_key("claude"),
            "claude provider should be present via defaults after parse failure"
        );
    }

    #[test]
    fn old_max_websocket_connections_key_still_loads_and_is_ignored() {
        // Back-compat: the removed `max_websocket_connections` key parses without
        // error because `ServerConfig` has no `#[serde(deny_unknown_fields)]` (TOML
        // simply ignores unknown keys; this is not a `serde(default)` effect), and
        // the three new split fields take their per-field defaults.
        let toml = r#"[server]
max_websocket_connections = 16
"#;
        let cfg: Config = toml::from_str(toml).expect("old config must still parse");
        assert_eq!(cfg.server.max_websocket_events_connections, 32);
        assert_eq!(cfg.server.max_websocket_agent_connections, 32);
        assert_eq!(cfg.server.max_websocket_terminal_connections, 64);
    }

    #[test]
    fn detects_removed_max_websocket_connections_key_for_migration_warning() {
        assert!(raw_has_removed_max_websocket_connections(
            "[server]\nmax_websocket_connections = 16\n"
        ));
        // A commented-out line must NOT trip the warning.
        assert!(!raw_has_removed_max_websocket_connections(
            "[server]\n# max_websocket_connections = 16\n"
        ));
        // The new split keys must NOT trip the warning.
        assert!(!raw_has_removed_max_websocket_connections(
            "[server]\nmax_websocket_events_connections = 16\n"
        ));
        assert!(!raw_has_removed_max_websocket_connections("[server]\n"));
    }
}

#[cfg(test)]
mod agent_tabs_cap_tests {
    use super::*;

    #[test]
    fn normalized_agent_tabs_max_substitutes_default_for_zero() {
        assert_eq!(normalized_agent_tabs_max(0), DEFAULT_AGENT_TABS_MAX);
    }

    #[test]
    fn normalized_agent_tabs_max_clamps_oversized_values() {
        assert_eq!(normalized_agent_tabs_max(10_000), MAX_AGENT_TABS_MAX);
    }

    #[test]
    fn normalized_agent_tabs_max_passes_through_sane_values() {
        assert_eq!(normalized_agent_tabs_max(8), 8);
    }

    #[test]
    fn pr_poll_interval_default_is_180() {
        assert_eq!(DEFAULT_PR_POLL_INTERVAL_SECONDS, 180);
        assert_eq!(
            UiConfig::default().pr_poll_interval_seconds,
            DEFAULT_PR_POLL_INTERVAL_SECONDS
        );
    }

    #[test]
    fn normalized_pr_poll_interval_preserves_zero_as_disabled() {
        // 0 is a valid "disable the blind poll" value, NOT substituted with a default.
        assert_eq!(normalized_pr_poll_interval(0), 0);
    }

    #[test]
    fn normalized_pr_poll_interval_clamps_oversized_values() {
        assert_eq!(
            normalized_pr_poll_interval(u16::MAX),
            MAX_PR_POLL_INTERVAL_SECONDS
        );
    }

    #[test]
    fn normalized_pr_poll_interval_passes_through_sane_values() {
        assert_eq!(normalized_pr_poll_interval(180), 180);
    }

    #[test]
    fn normalized_pr_poll_interval_clamps_small_nonzero_up_to_floor() {
        assert_eq!(normalized_pr_poll_interval(1), MIN_PR_POLL_INTERVAL_SECONDS);
        // The floor itself passes through unchanged.
        assert_eq!(
            normalized_pr_poll_interval(MIN_PR_POLL_INTERVAL_SECONDS),
            MIN_PR_POLL_INTERVAL_SECONDS
        );
    }

    #[test]
    fn normalized_pr_poll_interval_allows_exact_ceiling() {
        assert_eq!(
            normalized_pr_poll_interval(MAX_PR_POLL_INTERVAL_SECONDS),
            MAX_PR_POLL_INTERVAL_SECONDS
        );
    }

    // -----------------------------------------------------------------------
    // providers.<name>.web_dragdrop_paste
    // -----------------------------------------------------------------------

    #[test]
    fn shipped_providers_get_their_documented_web_dragdrop_paste() {
        let providers = ProvidersConfig::default();
        for (name, expected) in [
            ("claude", WebDragDropPaste::Bare),
            ("opencode", WebDragDropPaste::Bare),
            ("codex", WebDragDropPaste::SingleQuoted),
            ("copilot", WebDragDropPaste::Bare),
        ] {
            assert_eq!(
                providers
                    .get(name)
                    .expect("shipped provider present")
                    .resolved_web_dragdrop_paste(),
                expected,
                "{name} must ship with the measured paste form"
            );
        }
    }

    #[test]
    fn a_user_defined_provider_pastes_bare() {
        let config: Config =
            toml::from_str("[providers.myagent]\ncommand = \"myagent\"\n").expect("parse config");
        assert_eq!(
            config.providers.commands["myagent"].resolved_web_dragdrop_paste(),
            WebDragDropPaste::Bare,
            "a provider dux knows nothing about gets the do-nothing form"
        );
    }

    #[test]
    fn ensure_defaults_fills_a_missing_web_dragdrop_paste_for_a_shipped_provider() {
        let mut config: Config =
            toml::from_str("[providers.codex]\ncommand = \"codex\"\n").expect("parse config");
        assert_eq!(config.providers.commands["codex"].web_dragdrop_paste, None);
        config.providers.ensure_defaults();
        assert_eq!(
            config.providers.commands["codex"].resolved_web_dragdrop_paste(),
            WebDragDropPaste::SingleQuoted,
            "a config predating the setting must still get codex's measured form"
        );
    }

    #[test]
    fn an_explicit_web_dragdrop_paste_wins_over_the_shipped_default() {
        let mut config: Config = toml::from_str(
            "[providers.codex]\ncommand = \"codex\"\nweb_dragdrop_paste = \"bare\"\n",
        )
        .expect("parse config");
        config.providers.ensure_defaults();
        assert_eq!(
            config.providers.commands["codex"].resolved_web_dragdrop_paste(),
            WebDragDropPaste::Bare,
            "config wins for an explicit preference"
        );
    }

    #[test]
    fn a_provider_is_identified_by_its_command_file_name_not_its_block_name() {
        // The two are independent, and the per-CLI paste-length table used to be
        // keyed by the BLOCK NAME, which is free text. Both directions were
        // wrong: a real Codex under any other name escaped its limit and was
        // handed oversized paths it silently ignores, and an unrelated CLI
        // merely NAMED codex had valid long paths withheld from it.
        let config: Config = toml::from_str(
            "[providers.myagent]\ncommand = \"codex\"\n\
             [providers.codex]\ncommand = \"something-else\"\n",
        )
        .expect("parse config");
        assert_eq!(
            config.providers.commands["myagent"].command_file_name(),
            "codex",
            "an aliased block still runs codex, and must be identified as codex"
        );
        assert_eq!(
            config.providers.commands["codex"].command_file_name(),
            "something-else",
            "a block merely NAMED codex runs whatever its command says"
        );
    }

    #[test]
    fn a_full_path_command_is_identified_by_its_file_name() {
        // `command` may be an absolute path, and it names the same CLI as the
        // bare name does, so the comparison is on the file name.
        for command in ["/usr/local/bin/codex", "./codex", "codex"] {
            let config: Config =
                toml::from_str(&format!("[providers.p]\ncommand = \"{command}\"\n"))
                    .expect("parse config");
            assert_eq!(
                config.providers.commands["p"].command_file_name(),
                "codex",
                "{command} names codex"
            );
        }
    }

    #[test]
    fn a_command_with_no_file_name_falls_back_to_itself() {
        // Degenerate, but it must never resolve to an empty string, which would
        // key into a table as a real value.
        for command in ["", "/", ".."] {
            let config: Config =
                toml::from_str(&format!("[providers.p]\ncommand = \"{command}\"\n"))
                    .expect("parse config");
            let resolved = config.providers.commands["p"].command_file_name();
            assert_eq!(
                resolved, command,
                "a command with no file name answers with itself"
            );
        }
    }

    #[test]
    fn web_dragdrop_paste_parses_case_and_whitespace_insensitively() {
        assert_eq!(
            WebDragDropPaste::parse("  BARE "),
            Some(WebDragDropPaste::Bare)
        );
        assert_eq!(
            WebDragDropPaste::parse("Single_Quoted"),
            Some(WebDragDropPaste::SingleQuoted)
        );
        assert_eq!(
            WebDragDropPaste::parse("Double_Quoted"),
            Some(WebDragDropPaste::DoubleQuoted)
        );
        assert_eq!(
            WebDragDropPaste::parse(" backslash_escaped\n"),
            Some(WebDragDropPaste::BackslashEscaped)
        );
        assert_eq!(WebDragDropPaste::parse("shell"), None);
        assert_eq!(WebDragDropPaste::parse("file_url"), None);
    }

    #[test]
    fn a_misspelled_web_dragdrop_paste_falls_back_to_bare() {
        // Same shape as `capabilities.clipboard_passthrough`: an unrecognized
        // value degrades to the safe default rather than failing the load.
        let config: Config = toml::from_str(
            "[providers.codex]\ncommand = \"codex\"\nweb_dragdrop_paste = \"single-quoted\"\n",
        )
        .expect("a typo must not fail the whole config load");
        assert_eq!(
            config.providers.commands["codex"].resolved_web_dragdrop_paste(),
            WebDragDropPaste::Bare
        );
        assert_eq!(
            WebDragDropPaste::from_config_str("codex", "single-quoted"),
            WebDragDropPaste::Bare
        );
    }

    #[test]
    fn a_misspelled_web_dragdrop_paste_warns_once_per_load_and_says_what_is_valid() {
        // The test above asserts only the FALLBACK, and the fallback is the
        // silent half. Nothing looked at the warning at all, so a degradation
        // that told the user nothing would have passed just as happily, which is
        // the failure mode the warning exists to prevent: a typed
        // `single-quoted` stops quoting paths and nothing anywhere says why.
        //
        // The message is asserted directly rather than read back out of the log
        // file: `logger::init` sets a process-wide `OnceLock`, so whichever test
        // runs first owns it, and a test that read the file would pass or fail on
        // ordering. `web_dragdrop_paste_warnings` returns the exact strings
        // `load_config` hands to the logger, so this pins the text AND the count.
        let config: Config = toml::from_str(
            "[providers.claude]\ncommand = \"claude\"\nweb_dragdrop_paste = \"bare\"\n\
             [providers.codex]\ncommand = \"codex\"\nweb_dragdrop_paste = \"single-quoted\"\n\
             [providers.opencode]\ncommand = \"opencode\"\nweb_dragdrop_paste = \"file_url\"\n",
        )
        .expect("a typo must not fail the whole config load");

        let warnings = web_dragdrop_paste_warnings(&config.providers);
        // ONE per misspelled provider. Not one per provider, and not one per
        // dropped file.
        assert_eq!(
            warnings.len(),
            2,
            "expected exactly one warning per misspelled provider, got {warnings:?}"
        );
        let joined = warnings.join("\n");
        assert!(joined.contains("providers.codex.web_dragdrop_paste"));
        assert!(joined.contains("\"single-quoted\""));
        assert!(joined.contains("providers.opencode.web_dragdrop_paste"));
        assert!(joined.contains("\"file_url\""));
        // The correctly spelled provider is not mentioned at all.
        assert!(!joined.contains("providers.claude"));
        // And each warning says what happens instead and what would have worked,
        // because a warning that only says "unknown" leaves the user guessing.
        for warning in &warnings {
            assert!(warning.contains("falling back to \"bare\""), "{warning}");
            assert!(warning.contains("single_quoted"), "{warning}");
            assert!(warning.contains("double_quoted"), "{warning}");
            assert!(warning.contains("backslash_escaped"), "{warning}");
        }

        // The per-paste path resolves the same value as often as it likes and
        // produces no warning, which is what "once per load" means: the count is
        // a function of the config, not of how often the value has been read.
        for _ in 0..5 {
            assert_eq!(
                config.providers.commands["codex"].resolved_web_dragdrop_paste(),
                WebDragDropPaste::Bare
            );
        }
        assert_eq!(web_dragdrop_paste_warnings(&config.providers).len(), 2);

        // A clean config says nothing at all.
        assert!(web_dragdrop_paste_warnings(&ProvidersConfig::default()).is_empty());
    }

    #[test]
    fn web_dragdrop_paste_as_str_round_trips() {
        for mode in [
            WebDragDropPaste::Bare,
            WebDragDropPaste::SingleQuoted,
            WebDragDropPaste::DoubleQuoted,
            WebDragDropPaste::BackslashEscaped,
        ] {
            assert_eq!(WebDragDropPaste::parse(mode.as_str()), Some(mode));
        }
    }
}
