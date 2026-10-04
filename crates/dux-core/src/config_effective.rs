//! The value dux uses for every setting it parses, defaults or clamps where
//! the value is USED rather than when the file is loaded.
//!
//! One function per such setting, `effective_<setting>`, and it is the one
//! place that setting's rule lives. Every runtime reader goes through it; `dux
//! config get` reports what it returns, with why, through
//! [`use_time_corrections`]; and `dux config set` refuses a value of a
//! fixed-value setting its parser does not accept, through [`fixed_values`].
//! So what dux runs with, what `get` says it runs with and what `set` lets you
//! write cannot disagree.
//!
//! Every function here is pure. A value dux reads as something else is said
//! once, when the config is loaded (the load logs every reason
//! [`use_time_corrections`] gives), never on each read: several of these are
//! read every engine tick or on every bootstrap fetch.
//!
//! A value whose MEANING is the setting itself is not a correction and has no
//! entry here: `0` that switches something off, unlimits it or means "keep
//! trying forever" is read as written. A value dux uses in place of the one
//! written is, even when the config comment says so ("0 falls back to the
//! default", "clamped to 600").

use serde::Serialize;

use crate::config::{
    CapabilitiesConfig, ClipboardPassthroughMode, ComposeBarMode, Config, DEFAULT_AGENT_TABS_MAX,
    DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS, DEFAULT_HEARTBEAT_DEADLINE_SECONDS,
    DEFAULT_HEARTBEAT_SECONDS, DEFAULT_PTY_SEND_TIMEOUT_SECONDS,
    DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS, DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS,
    DEFAULT_UPLOAD_DIRECTORY, LOG_VIEWER_LINES_MAX, MAX_AGENT_TABS_MAX, MAX_LOG_KEEP,
    MAX_PR_POLL_INACTIVE_INTERVAL_SECONDS, MAX_PR_POLL_INTERVAL_SECONDS,
    MAX_SHUTDOWN_TIMEOUT_SECONDS, MIN_PR_POLL_INTERVAL_SECONDS, TailscaleMode, WebDragDropPaste,
};
use crate::flat_list::FlatSortMode;
use crate::term_identity::TerminalIdentityMode;

// ---------------------------------------------------------------------------
// [logging]
// ---------------------------------------------------------------------------

/// A log level dux writes at, ordered from the fewest lines to the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    /// Every level, in the order the config comment lists them.
    pub const ALL: [Self; 4] = [Self::Debug, Self::Info, Self::Warn, Self::Error];

    /// The level a config value names, written exactly as [`Self::as_str`]
    /// spells it, or `None`.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == value)
    }

    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    /// The label a log line carries.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }

    /// The inverse of the `as u8` cast the logger stores the level as.
    /// Anything else means a corrupted store, which reads as the default.
    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Error as u8 => Self::Error,
            v if v == Self::Warn as u8 => Self::Warn,
            v if v == Self::Debug as u8 => Self::Debug,
            _ => Self::Info,
        }
    }
}

/// `[logging] level`: the level it names, and `info` for anything else.
pub fn effective_log_level(configured: &str) -> LogLevel {
    LogLevel::parse(configured).unwrap_or(LogLevel::Info)
}

/// `[logging] keep`: at most [`MAX_LOG_KEEP`] rotated copies. `0` is a real
/// answer (rotate and discard).
pub fn effective_log_keep(configured: u32) -> u32 {
    configured.min(MAX_LOG_KEEP)
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

/// `shutdown_timeout_seconds` (top level, for the terminal UI's quit) and
/// `[server] shutdown_timeout_seconds`: at most
/// [`MAX_SHUTDOWN_TIMEOUT_SECONDS`]. The field is seconds, but a `u16` reaches
/// about 18 hours, so a value meant as milliseconds would otherwise block quit
/// for that long.
pub fn effective_shutdown_timeout_seconds(configured: u16) -> u16 {
    configured.min(MAX_SHUTDOWN_TIMEOUT_SECONDS)
}

// ---------------------------------------------------------------------------
// [ui]
// ---------------------------------------------------------------------------

/// `[ui] agent_tabs_max`: `0` means the default; above
/// [`MAX_AGENT_TABS_MAX`] is that ceiling.
pub fn effective_agent_tabs_max(configured: u16) -> u16 {
    if configured == 0 {
        return DEFAULT_AGENT_TABS_MAX;
    }
    configured.min(MAX_AGENT_TABS_MAX)
}

/// `[ui] pr_poll_interval_seconds`: `0` turns the blind poll off; anything
/// else is held in [`MIN_PR_POLL_INTERVAL_SECONDS`]..=
/// [`MAX_PR_POLL_INTERVAL_SECONDS`], so a mistyped tiny value cannot hammer
/// the GitHub API and a huge one cannot quietly neuter the backstop.
pub fn effective_pr_poll_interval_seconds(configured: u16) -> u16 {
    if configured == 0 {
        return 0;
    }
    configured.clamp(MIN_PR_POLL_INTERVAL_SECONDS, MAX_PR_POLL_INTERVAL_SECONDS)
}

/// `[ui] pr_poll_inactive_interval_seconds`: `0` never polls an inactive
/// agent; anything else is held between the active poll's floor (the floor
/// is about not hammering the API, whichever clock an agent is on) and
/// [`MAX_PR_POLL_INACTIVE_INTERVAL_SECONDS`].
pub fn effective_pr_poll_inactive_interval_seconds(configured: u32) -> u32 {
    if configured == 0 {
        return 0;
    }
    configured.clamp(
        u32::from(MIN_PR_POLL_INTERVAL_SECONDS),
        MAX_PR_POLL_INACTIVE_INTERVAL_SECONDS,
    )
}

/// The least share of the terminal UI's height a resizable pane takes.
pub const MIN_PANE_HEIGHT_PCT: u16 = 10;
/// The most share of the terminal UI's height a resizable pane takes.
pub const MAX_PANE_HEIGHT_PCT: u16 = 80;

/// `[ui] terminal_pane_height_pct`, `staged_pane_height_pct` and
/// `commit_pane_height_pct`: held in [`MIN_PANE_HEIGHT_PCT`]..=
/// [`MAX_PANE_HEIGHT_PCT`], the same bounds a resize with the keyboard or
/// the mouse stops at.
pub fn effective_pane_height_pct(configured: u16) -> u16 {
    configured.clamp(MIN_PANE_HEIGHT_PCT, MAX_PANE_HEIGHT_PCT)
}

/// Every `[ui] agent_sort` value, with the order it names.
pub const AGENT_SORTS: [(&str, FlatSortMode); 6] = [
    ("active", FlatSortMode::Active),
    ("updated", FlatSortMode::Updated),
    ("created", FlatSortMode::Created),
    ("name", FlatSortMode::NameAsc),
    ("name_desc", FlatSortMode::NameDesc),
    ("manual", FlatSortMode::Manual),
];

/// The order an `[ui] agent_sort` value names, written exactly, or `None`.
pub fn parse_agent_sort(value: &str) -> Option<FlatSortMode> {
    AGENT_SORTS
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, mode)| *mode)
}

/// The config spelling of an agent order.
pub fn agent_sort_name(mode: FlatSortMode) -> &'static str {
    AGENT_SORTS
        .iter()
        .find(|(_, known)| *known == mode)
        .map_or("active", |(name, _)| name)
}

/// `[ui] agent_sort`: the order it names, and `active` (the default) for
/// anything else, on both surfaces.
pub fn effective_agent_sort(configured: &str) -> FlatSortMode {
    parse_agent_sort(configured).unwrap_or(FlatSortMode::Active)
}

/// Where the pull-request banner sits in an agent's pane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrBannerPosition {
    Top,
    /// The default.
    #[default]
    Bottom,
}

impl PrBannerPosition {
    /// Every position, in the order the config comment lists them.
    pub const ALL: [Self; 2] = [Self::Top, Self::Bottom];

    /// The position a config value names, read without regard to case or
    /// surrounding spaces, or `None`.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|position| position.as_str() == value)
    }

    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    /// The other position, for the toggle.
    pub fn flipped(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

/// `[ui] pr_banner_position`: the position it names, and the default
/// (bottom) for anything else.
pub fn effective_pr_banner_position(configured: &str) -> PrBannerPosition {
    PrBannerPosition::parse(configured).unwrap_or_default()
}

/// `[ui] compose_bar`: the mode it names, read without regard to case or
/// surrounding spaces, and `auto` for anything else (which the load also
/// corrects, with a warning of its own).
pub fn effective_compose_bar(configured: &str) -> ComposeBarMode {
    ComposeBarMode::parse(configured).unwrap_or_default()
}

/// `[ui] upload_directory`: the relative path dux creates, its components
/// rejoined without `.` and without surrounding spaces, or
/// [`DEFAULT_UPLOAD_DIRECTORY`] when the value is unusable (which the load
/// also corrects, with a warning naming why).
pub fn effective_upload_directory(configured: &str) -> String {
    if crate::config::upload_directory_rejection(configured).is_some() {
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

/// The longest `[ui] terminal_font_family` the web UI uses.
pub const MAX_TERMINAL_FONT_FAMILY_CHARS: usize = 200;

/// Whether `c` may stay in a terminal font family name. The name is
/// concatenated into a CSS `font-family` declaration and into the font
/// loader's shorthand, so only a class narrow enough to prove safe stays:
/// non-ASCII names (accented Latin, CJK) are taken out, and the bundled fonts
/// stand in.
fn font_family_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-' | ',' | '\'' | '"')
}

/// `[ui] terminal_font_family`: the name the web terminal puts ahead of its
/// bundled fonts, with every character outside the safe class taken out, at
/// most [`MAX_TERMINAL_FONT_FAMILY_CHARS`] characters, and no surrounding
/// spaces. Empty means the bundled fonts alone. The browser's own guard at
/// the CSS sink reads any value this returns as it is (a shared fixture pins
/// the two).
pub fn effective_terminal_font_family(configured: &str) -> String {
    let kept: String = configured
        .trim()
        .chars()
        .filter(|c| font_family_char(*c))
        .take(MAX_TERMINAL_FONT_FAMILY_CHARS)
        .collect();
    kept.trim().to_string()
}

// ---------------------------------------------------------------------------
// [capabilities]
// ---------------------------------------------------------------------------

/// `[capabilities] terminal_identity`: the mode it names, read without regard
/// to case or surrounding spaces, and `auto` for anything else.
pub fn effective_terminal_identity(configured: &str) -> TerminalIdentityMode {
    TerminalIdentityMode::parse(configured).unwrap_or(TerminalIdentityMode::Auto)
}

/// `[capabilities] clipboard_passthrough`: `off` while `passthrough` (the
/// master switch over every passthrough) is off; otherwise the mode it names,
/// read without regard to case or surrounding spaces, and `focused` for
/// anything else.
pub fn effective_clipboard_passthrough(
    capabilities: &CapabilitiesConfig,
) -> ClipboardPassthroughMode {
    if !capabilities.passthrough {
        return ClipboardPassthroughMode::Off;
    }
    ClipboardPassthroughMode::parse(&capabilities.clipboard_passthrough)
        .unwrap_or(ClipboardPassthroughMode::Focused)
}

// ---------------------------------------------------------------------------
// [providers.<name>]
// ---------------------------------------------------------------------------

/// `[providers.<name>] web_dragdrop_paste`: the form it names, read without
/// regard to case or surrounding spaces, and `bare` when it is left out or
/// names no form.
pub fn effective_web_dragdrop_paste(configured: Option<&str>) -> WebDragDropPaste {
    configured
        .and_then(WebDragDropPaste::parse)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// [editor]
// ---------------------------------------------------------------------------

/// `[editor] default`: the config key of the editor it names (by its key,
/// an alias or one of its commands), or `None` when it names none, in which
/// case dux opens the first supported editor it finds on PATH.
pub fn effective_editor_default(configured: &str) -> Option<&'static str> {
    crate::editor::configured_editor_key(configured)
}

// ---------------------------------------------------------------------------
// [server]
// ---------------------------------------------------------------------------

/// `[server] tailscale`: the mode it names, read without regard to case or
/// surrounding spaces, and `auto` for anything else (which the load also
/// corrects, with a warning of its own).
pub fn effective_tailscale(configured: &str) -> TailscaleMode {
    TailscaleMode::parse(configured).unwrap_or_default()
}

/// Whether `dux server` colors its console.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerColor {
    /// Color when the console is a terminal that wants it. The default.
    #[default]
    Auto,
    Always,
    Never,
}

impl ServerColor {
    /// Every value, in the order the config comment lists them.
    pub const ALL: [Self; 3] = [Self::Auto, Self::Always, Self::Never];

    /// The value a config string names, written exactly, or `None`.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.as_str() == value)
    }

    /// The config spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// `[server] color`: the value it names, and `auto` for anything else.
pub fn effective_server_color(configured: &str) -> ServerColor {
    ServerColor::parse(configured).unwrap_or_default()
}

/// `[server] log_viewer_lines`: at least 1, at most
/// [`LOG_VIEWER_LINES_MAX`].
pub fn effective_log_viewer_lines(configured: usize) -> usize {
    configured.clamp(1, LOG_VIEWER_LINES_MAX)
}

/// `[server] file_drop_max_concurrency`: at least 1, since no slots at all
/// would stall every drop forever (`file_drop_max_bytes = 0` is how file drop
/// is switched off).
pub fn effective_file_drop_max_concurrency(configured: u32) -> u32 {
    configured.max(1)
}

/// `0` read as `default`, for a timing where zero would mean a hot loop or
/// an instant failure rather than "off".
fn nonzero_or(configured: u32, default: u32) -> u32 {
    if configured == 0 { default } else { configured }
}

/// `[server] pty_send_timeout_seconds`: `0` means the default.
pub fn effective_pty_send_timeout_seconds(configured: u32) -> u32 {
    nonzero_or(configured, DEFAULT_PTY_SEND_TIMEOUT_SECONDS)
}

/// `[server] heartbeat_seconds`: `0` means the default.
pub fn effective_heartbeat_seconds(configured: u32) -> u32 {
    nonzero_or(configured, DEFAULT_HEARTBEAT_SECONDS)
}

/// How many beat periods a deadline at or below the period is raised to.
const INVERTED_DEADLINE_PERIODS: u32 = 2;

/// `[server] heartbeat_deadline_seconds`, beside `heartbeat_seconds`: `0`
/// means the default, and a deadline at or below the (effective) period is
/// twice the period. The deadline is checked on the beat's own timer, so one
/// at or below it would find itself passed on the first tick and drop a
/// healthy connection over and over.
pub fn effective_heartbeat_deadline_seconds(configured: u32, heartbeat_seconds: u32) -> u32 {
    let deadline = nonzero_or(configured, DEFAULT_HEARTBEAT_DEADLINE_SECONDS);
    let period = effective_heartbeat_seconds(heartbeat_seconds);
    if deadline > period {
        deadline
    } else {
        period.saturating_mul(INVERTED_DEADLINE_PERIODS)
    }
}

/// `[server] reconnect_backoff_cap_seconds`: `0` means the default.
pub fn effective_reconnect_backoff_cap_seconds(configured: u32) -> u32 {
    nonzero_or(configured, DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS)
}

/// `[server] reconnect_attempt_timeout_seconds`: `0` means the default.
pub fn effective_reconnect_attempt_timeout_seconds(configured: u32) -> u32 {
    nonzero_or(configured, DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS)
}

/// The longest the web UI's Changes pane waits for its list.
pub const MAX_CHANGES_REQUEST_TIMEOUT_SECONDS: u32 = 600;

/// `[server] changes_request_timeout_seconds`: `0` means the default, and
/// above [`MAX_CHANGES_REQUEST_TIMEOUT_SECONDS`] is that ceiling.
pub fn effective_changes_request_timeout_seconds(configured: u32) -> u32 {
    nonzero_or(configured, DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS)
        .min(MAX_CHANGES_REQUEST_TIMEOUT_SECONDS)
}

/// The longest `[server] title` dux shows, in characters.
pub const MAX_SERVER_TITLE_CHARS: usize = 200;

/// True for a character that must not survive into a title: a Unicode control
/// code (category Cc, via `char::is_control`) OR one of the bidi/format
/// characters (a subset of Cf) that can visually reorder or hide text.
/// `char::is_control` alone misses Cf, so a right-to-left override (U+202E) or
/// zero-width joiner would pass through and spoof the rendered tab title and the
/// on-disk `config.toml`, a Trojan-Source-style display attack.
fn is_unsafe_title_char(ch: char) -> bool {
    ch.is_control()
        || matches!(ch,
            // Zero-width + directional marks, bidi embeddings/overrides, bidi
            // isolates, and the byte-order mark / zero-width no-break space.
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}')
}

/// `[server] title`: every unsafe character (Unicode control codes and
/// bidi/format characters; see [`is_unsafe_title_char`]) replaced with a
/// space, runs of whitespace collapsed to one, trimmed, and at most
/// [`MAX_SERVER_TITLE_CHARS`] characters (counted by `char`, never bytes). An
/// empty result is the default, `dux`.
pub fn effective_server_title(configured: &str) -> String {
    let mut collapsed = String::new();
    let mut pending_space = false;
    for ch in configured.chars() {
        let ch = if is_unsafe_title_char(ch) { ' ' } else { ch };
        if ch == ' ' {
            // Deferred so runs collapse and leading space drops.
            if !collapsed.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                collapsed.push(' ');
                pending_space = false;
            }
            collapsed.push(ch);
        }
    }
    let capped: String = collapsed.chars().take(MAX_SERVER_TITLE_CHARS).collect();
    let trimmed = capped.trim();
    if trimmed.is_empty() {
        "dux".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The curated favicon TINT color names `[server] favicon` accepts. The
/// default (an empty value) is the original full-color yellow duck; these
/// names recolor a flat duck silhouette instead, so `yellow` is intentionally
/// NOT a tint. This is the CANONICAL list; the web frontend mirrors it. Keep
/// the two in sync.
pub const CURATED_FAVICON_COLORS: &[&str] = &[
    "violet", "blue", "sky", "cyan", "teal", "green", "amber", "orange", "red", "pink", "rose",
];

/// The favicon a `[server] favicon` value names, read without regard to case
/// or surrounding spaces: a curated tint, or the empty string for the default
/// duck. `None` for anything else.
pub fn parse_server_favicon(value: &str) -> Option<String> {
    let normalized = value.trim().to_lowercase();
    if normalized.is_empty() || CURATED_FAVICON_COLORS.contains(&normalized.as_str()) {
        Some(normalized)
    } else {
        None
    }
}

/// `[server] favicon`: the tint it names, and the default duck (empty) for
/// anything else.
pub fn effective_server_favicon(configured: &str) -> String {
    parse_server_favicon(configured).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// What `get` reports
// ---------------------------------------------------------------------------

/// A setting dux uses as something other than what the file writes, found
/// by its `effective_*` function.
#[derive(Clone, Debug, PartialEq)]
pub struct UseTimeCorrection {
    /// The setting, as segments.
    pub path: Vec<String>,
    /// What dux uses.
    pub used: serde_json::Value,
    /// Why, in words that repeat nothing the file wrote: the value may be a
    /// token pasted into the wrong setting.
    pub reason: String,
}

/// Every setting of `config` dux uses as something other than what it says,
/// each with what it uses and why. `get` reports these beside what the file
/// says, and the load logs each reason once.
pub fn use_time_corrections(config: &Config) -> Vec<UseTimeCorrection> {
    let mut found = Vec::new();
    let mut note = |path: &[&str],
                    written: serde_json::Value,
                    used: serde_json::Value,
                    reason: &dyn Fn() -> String| {
        if written != used {
            found.push(UseTimeCorrection {
                path: path.iter().map(|segment| (*segment).to_string()).collect(),
                used,
                reason: reason(),
            });
        }
    };
    let json = |value: &dyn ErasedSerialize| value.to_json();
    let numbered = |configured: u64, used: u64, ceiling: u64, floor_reason: &str| -> String {
        if configured > used && used == ceiling {
            format!("dux uses at most {ceiling}")
        } else {
            floor_reason.to_string()
        }
    };
    // A value its parser reads (only the spelling differs from the config
    // comment's), or one it does not.
    let worded = |parsed: bool, known: &str, fallback: &str| -> String {
        if parsed {
            "dux reads this setting without regard to case or surrounding spaces".to_string()
        } else {
            format!("dux knows {known}, and uses {fallback} for anything else")
        }
    };

    let level = &config.logging.level;
    note(
        &["logging", "level"],
        json(level),
        json(&effective_log_level(level).as_str()),
        &|| {
            "dux knows the levels debug, info, warn and error, written in lowercase, and logs at info for anything else".to_string()
        },
    );
    let keep = config.logging.keep;
    note(
        &["logging", "keep"],
        json(&keep),
        json(&effective_log_keep(keep)),
        &|| format!("dux keeps at most {MAX_LOG_KEEP} rotated copies"),
    );
    for (path, seconds) in [
        (
            &["shutdown_timeout_seconds"][..],
            config.shutdown_timeout_seconds,
        ),
        (
            &["server", "shutdown_timeout_seconds"][..],
            config.server.shutdown_timeout_seconds,
        ),
    ] {
        note(
            path,
            json(&seconds),
            json(&effective_shutdown_timeout_seconds(seconds)),
            &|| {
                format!(
                    "dux waits at most {MAX_SHUTDOWN_TIMEOUT_SECONDS} seconds for agents and terminals \
                 to exit (the value is in seconds, not milliseconds)"
                )
            },
        );
    }

    let ui = &config.ui;
    let tabs = ui.agent_tabs_max;
    note(
        &["ui", "agent_tabs_max"],
        json(&tabs),
        json(&effective_agent_tabs_max(tabs)),
        &|| {
            numbered(
                u64::from(tabs),
                u64::from(effective_agent_tabs_max(tabs)),
                u64::from(MAX_AGENT_TABS_MAX),
                "0 means the default",
            )
        },
    );
    let poll = ui.pr_poll_interval_seconds;
    note(
        &["ui", "pr_poll_interval_seconds"],
        json(&poll),
        json(&effective_pr_poll_interval_seconds(poll)),
        &|| {
            numbered(
                u64::from(poll),
                u64::from(effective_pr_poll_interval_seconds(poll)),
                u64::from(MAX_PR_POLL_INTERVAL_SECONDS),
                &format!(
                    "a poll runs at most every {MIN_PR_POLL_INTERVAL_SECONDS} seconds; 0 turns it off"
                ),
            )
        },
    );
    let inactive = ui.pr_poll_inactive_interval_seconds;
    note(
        &["ui", "pr_poll_inactive_interval_seconds"],
        json(&inactive),
        json(&effective_pr_poll_inactive_interval_seconds(inactive)),
        &|| {
            numbered(
                u64::from(inactive),
                u64::from(effective_pr_poll_inactive_interval_seconds(inactive)),
                u64::from(MAX_PR_POLL_INACTIVE_INTERVAL_SECONDS),
                &format!(
                    "a poll runs at most every {MIN_PR_POLL_INTERVAL_SECONDS} seconds; 0 turns it off"
                ),
            )
        },
    );
    for (key, pct) in [
        ("terminal_pane_height_pct", ui.terminal_pane_height_pct),
        ("staged_pane_height_pct", ui.staged_pane_height_pct),
        ("commit_pane_height_pct", ui.commit_pane_height_pct),
    ] {
        note(
            &["ui", key],
            json(&pct),
            json(&effective_pane_height_pct(pct)),
            &|| {
                format!(
                    "a pane takes between {MIN_PANE_HEIGHT_PCT} and {MAX_PANE_HEIGHT_PCT} percent of \
                 the height"
                )
            },
        );
    }
    note(
        &["ui", "agent_sort"],
        json(&ui.agent_sort),
        json(&agent_sort_name(effective_agent_sort(&ui.agent_sort))),
        &|| {
            "dux knows the sorts active, updated, created, name, name_desc and manual, and sorts \
             by active for anything else"
                .to_string()
        },
    );
    let position = &ui.pr_banner_position;
    note(
        &["ui", "pr_banner_position"],
        json(position),
        json(&effective_pr_banner_position(position).as_str()),
        &|| {
            worded(
                PrBannerPosition::parse(position).is_some(),
                "top and bottom",
                "bottom",
            )
        },
    );
    let compose = &ui.compose_bar;
    note(
        &["ui", "compose_bar"],
        json(compose),
        json(&effective_compose_bar(compose).as_str()),
        &|| {
            worded(
                ComposeBarMode::parse(compose).is_some(),
                "auto, always and never",
                "auto",
            )
        },
    );
    let uploads = &ui.upload_directory;
    note(
        &["ui", "upload_directory"],
        json(uploads),
        json(&effective_upload_directory(uploads)),
        &|| {
            if crate::config::upload_directory_rejection(uploads).is_some() {
                format!(
                    "dux cannot use it as a directory inside the worktree, so it uses {DEFAULT_UPLOAD_DIRECTORY}"
                )
            } else {
                "dux creates the path without its . components or surrounding spaces".to_string()
            }
        },
    );
    let family = &ui.terminal_font_family;
    note(
        &["ui", "terminal_font_family"],
        json(family),
        json(&effective_terminal_font_family(family)),
        &|| {
            format!(
                "the web terminal keeps only letters, digits, spaces and _ - , ' \" in a font \
                 name, at most {MAX_TERMINAL_FONT_FAMILY_CHARS} characters, without surrounding \
                 spaces"
            )
        },
    );

    let capabilities = &config.capabilities;
    let identity = &capabilities.terminal_identity;
    note(
        &["capabilities", "terminal_identity"],
        json(identity),
        json(&effective_terminal_identity(identity).as_str()),
        &|| {
            worded(
                TerminalIdentityMode::parse(identity).is_some(),
                "auto, mirror, ghostty, kitty, iterm2 and none",
                "auto",
            )
        },
    );
    let clipboard = &capabilities.clipboard_passthrough;
    note(
        &["capabilities", "clipboard_passthrough"],
        json(clipboard),
        json(&effective_clipboard_passthrough(capabilities).as_str()),
        &|| {
            if !capabilities.passthrough {
                "capabilities.passthrough = false turns every passthrough off, the clipboard's \
                 included"
                    .to_string()
            } else {
                worded(
                    ClipboardPassthroughMode::parse(clipboard).is_some(),
                    "focused, always and off",
                    "focused",
                )
            }
        },
    );

    for (name, provider) in &config.providers.commands {
        let Some(form) = provider.web_dragdrop_paste.as_deref() else {
            continue;
        };
        note(
            &["providers", name, "web_dragdrop_paste"],
            json(&form),
            json(&effective_web_dragdrop_paste(Some(form)).as_str()),
            &|| {
                worded(
                    WebDragDropPaste::parse(form).is_some(),
                    "bare, single_quoted, double_quoted and backslash_escaped",
                    "bare",
                )
            },
        );
    }

    let editor = &config.editor.default;
    if !editor.trim().is_empty() && effective_editor_default(editor).is_none() {
        note(&["editor", "default"], json(editor), json(&""), &|| {
            "dux knows no editor by this name, so it opens the first supported editor it finds \
             on PATH"
                .to_string()
        });
    }

    let server = &config.server;
    note(
        &["server", "tailscale"],
        json(&server.tailscale),
        json(&effective_tailscale(&server.tailscale).as_str()),
        &|| {
            worded(
                TailscaleMode::parse(&server.tailscale).is_some(),
                "auto, yes and no",
                "auto",
            )
        },
    );
    note(
        &["server", "color"],
        json(&server.color),
        json(&effective_server_color(&server.color).as_str()),
        &|| {
            "dux knows auto, always and never, written in lowercase, and uses auto for anything \
             else"
                .to_string()
        },
    );
    let lines = server.log_viewer_lines;
    note(
        &["server", "log_viewer_lines"],
        json(&lines),
        json(&effective_log_viewer_lines(lines)),
        &|| format!("the log viewer keeps at least 1 line and at most {LOG_VIEWER_LINES_MAX}"),
    );
    let drops = server.file_drop_max_concurrency;
    note(
        &["server", "file_drop_max_concurrency"],
        json(&drops),
        json(&effective_file_drop_max_concurrency(drops)),
        &|| {
            "no upload slots at all would stall every drop, so 0 is 1 (file_drop_max_bytes = 0 \
             switches file drop off)"
                .to_string()
        },
    );
    for (key, configured, used) in [
        (
            "pty_send_timeout_seconds",
            server.pty_send_timeout_seconds,
            effective_pty_send_timeout_seconds(server.pty_send_timeout_seconds),
        ),
        (
            "heartbeat_seconds",
            server.heartbeat_seconds,
            effective_heartbeat_seconds(server.heartbeat_seconds),
        ),
        (
            "reconnect_backoff_cap_seconds",
            server.reconnect_backoff_cap_seconds,
            effective_reconnect_backoff_cap_seconds(server.reconnect_backoff_cap_seconds),
        ),
        (
            "reconnect_attempt_timeout_seconds",
            server.reconnect_attempt_timeout_seconds,
            effective_reconnect_attempt_timeout_seconds(server.reconnect_attempt_timeout_seconds),
        ),
    ] {
        note(&["server", key], json(&configured), json(&used), &|| {
            "0 means the default".to_string()
        });
    }
    let changes = server.changes_request_timeout_seconds;
    note(
        &["server", "changes_request_timeout_seconds"],
        json(&changes),
        json(&effective_changes_request_timeout_seconds(changes)),
        &|| {
            numbered(
                u64::from(changes),
                u64::from(effective_changes_request_timeout_seconds(changes)),
                u64::from(MAX_CHANGES_REQUEST_TIMEOUT_SECONDS),
                "0 means the default",
            )
        },
    );
    let deadline = server.heartbeat_deadline_seconds;
    note(
        &["server", "heartbeat_deadline_seconds"],
        json(&deadline),
        json(&effective_heartbeat_deadline_seconds(
            deadline,
            server.heartbeat_seconds,
        )),
        &|| {
            if deadline == 0
                && DEFAULT_HEARTBEAT_DEADLINE_SECONDS
                    > effective_heartbeat_seconds(server.heartbeat_seconds)
            {
                "0 means the default".to_string()
            } else {
                "a deadline at or below heartbeat_seconds would reconnect over and over, so dux \
                 uses twice heartbeat_seconds"
                    .to_string()
            }
        },
    );
    note(
        &["server", "title"],
        json(&server.title),
        json(&effective_server_title(&server.title)),
        &|| {
            format!(
                "dux shows the title with control and direction characters taken out, spaces \
                 collapsed, at most {MAX_SERVER_TITLE_CHARS} characters, and dux for an empty one"
            )
        },
    );
    note(
        &["server", "favicon"],
        json(&server.favicon),
        json(&effective_server_favicon(&server.favicon)),
        &|| {
            worded(
                parse_server_favicon(&server.favicon).is_some(),
                "the tints violet, blue, sky, cyan, teal, green, amber, orange, red, pink and \
                 rose",
                "the default duck",
            )
        },
    );
    found
}

/// A value serialized for comparison, without naming its type at the call.
trait ErasedSerialize {
    fn to_json(&self) -> serde_json::Value;
}

impl<T: Serialize + ?Sized> ErasedSerialize for T {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

// ---------------------------------------------------------------------------
// What `set` accepts
// ---------------------------------------------------------------------------

/// A setting with a fixed set of values: the values, as `set` lists them,
/// and the parser that decides what it accepts, which is the one its
/// `effective_*` function (or the load) reads the setting with.
pub struct FixedValues {
    /// The values, spelled as the config comment spells them.
    pub listed: Vec<&'static str>,
    /// Whether an empty value is accepted too (it means the default).
    pub or_empty: bool,
    accepts: fn(&str) -> bool,
}

impl FixedValues {
    /// Whether `value` is one dux reads as written.
    pub fn accepts(&self, value: &str) -> bool {
        (self.accepts)(value)
    }
}

/// The fixed values of the setting at `path`, if it has a fixed set.
pub fn fixed_values(path: &[String]) -> Option<FixedValues> {
    let parts: Vec<&str> = path.iter().map(String::as_str).collect();
    let fixed = |listed: Vec<&'static str>, accepts: fn(&str) -> bool| FixedValues {
        listed,
        or_empty: false,
        accepts,
    };
    Some(match parts.as_slice() {
        ["logging", "level"] => fixed(
            LogLevel::ALL.iter().map(|level| level.as_str()).collect(),
            |value| LogLevel::parse(value).is_some(),
        ),
        ["ui", "agent_sort"] => fixed(
            AGENT_SORTS.iter().map(|(name, _)| *name).collect(),
            |value| parse_agent_sort(value).is_some(),
        ),
        ["ui", "pr_banner_position"] => fixed(
            PrBannerPosition::ALL.iter().map(|p| p.as_str()).collect(),
            |value| PrBannerPosition::parse(value).is_some(),
        ),
        ["ui", "compose_bar"] => fixed(vec!["auto", "always", "never"], |value| {
            ComposeBarMode::parse(value).is_some()
        }),
        ["capabilities", "terminal_identity"] => fixed(
            vec!["auto", "mirror", "ghostty", "kitty", "iterm2", "none"],
            |value| TerminalIdentityMode::parse(value).is_some(),
        ),
        ["capabilities", "clipboard_passthrough"] => {
            fixed(vec!["focused", "always", "off"], |value| {
                ClipboardPassthroughMode::parse(value).is_some()
            })
        }
        ["providers", _, "web_dragdrop_paste"] => fixed(
            vec![
                "bare",
                "single_quoted",
                "double_quoted",
                "backslash_escaped",
            ],
            |value| WebDragDropPaste::parse(value).is_some(),
        ),
        ["editor", "default"] => FixedValues {
            listed: crate::editor::editor_config_keys(),
            or_empty: true,
            accepts: |value| value.trim().is_empty() || effective_editor_default(value).is_some(),
        },
        ["server", "tailscale"] => fixed(vec!["auto", "yes", "no"], |value| {
            TailscaleMode::parse(value).is_some()
        }),
        ["server", "color"] => fixed(
            ServerColor::ALL.iter().map(|c| c.as_str()).collect(),
            |value| ServerColor::parse(value).is_some(),
        ),
        ["server", "favicon"] => FixedValues {
            listed: CURATED_FAVICON_COLORS.to_vec(),
            or_empty: true,
            accepts: |value| parse_server_favicon(value).is_some(),
        },
        ["server", "auth", "require"] => fixed(
            crate::config_auth::AuthRequire::ALL
                .iter()
                .map(|v| v.as_str())
                .collect(),
            |value| {
                crate::config_auth::AuthRequire::ALL
                    .iter()
                    .any(|v| v.as_str() == value)
            },
        ),
        ["server", "auth", "cookie_secure"] => fixed(
            crate::config_auth::CookieSecure::ALL
                .iter()
                .map(|v| v.as_str())
                .collect(),
            |value| {
                crate::config_auth::CookieSecure::ALL
                    .iter()
                    .any(|v| v.as_str() == value)
            },
        ),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corrections_of(raw: &str) -> Vec<UseTimeCorrection> {
        let config = crate::config::effective_config_from_text(raw).expect("loads");
        use_time_corrections(&config)
    }

    fn used_at(raw: &str, path: &[&str]) -> Option<serde_json::Value> {
        corrections_of(raw)
            .into_iter()
            .find(|correction| correction.path == path)
            .map(|correction| correction.used)
    }

    #[test]
    fn a_level_the_logger_does_not_know_is_info_and_the_logger_agrees() {
        assert_eq!(effective_log_level("DEBUG"), LogLevel::Info);
        assert_eq!(effective_log_level("debug"), LogLevel::Debug);
        assert_eq!(effective_log_level(" warn"), LogLevel::Info);
        assert_eq!(
            used_at("[logging]\nlevel = \"DEBUG\"\n", &["logging", "level"]),
            Some(serde_json::json!("info"))
        );
        assert_eq!(
            used_at("[logging]\nlevel = \"warn\"\n", &["logging", "level"]),
            None
        );
    }

    #[test]
    fn the_coordinators_examples_are_each_reported_as_what_dux_uses() {
        assert_eq!(
            used_at("[ui]\nagent_sort = \"bogus\"\n", &["ui", "agent_sort"]),
            Some(serde_json::json!("active"))
        );
        assert_eq!(
            used_at(
                "[capabilities]\nclipboard_passthrough = \"sometimes\"\n",
                &["capabilities", "clipboard_passthrough"]
            ),
            Some(serde_json::json!("focused"))
        );
        assert_eq!(
            used_at(
                "[capabilities]\npassthrough = false\nclipboard_passthrough = \"always\"\n",
                &["capabilities", "clipboard_passthrough"]
            ),
            Some(serde_json::json!("off"))
        );
        assert_eq!(
            used_at(
                "shutdown_timeout_seconds = 9999\n",
                &["shutdown_timeout_seconds"]
            ),
            Some(serde_json::json!(600))
        );
        assert_eq!(
            used_at(
                "[server]\nshutdown_timeout_seconds = 9999\n",
                &["server", "shutdown_timeout_seconds"]
            ),
            Some(serde_json::json!(600))
        );
    }

    #[test]
    fn a_value_read_as_written_has_no_correction() {
        assert!(corrections_of("").is_empty(), "{:?}", corrections_of(""));
    }

    #[test]
    fn no_reason_repeats_a_value_the_file_wrote() {
        const TOKEN: &str = "zzTOKENabc123";
        let raw = format!(
            "[logging]\nlevel = \"{TOKEN}\"\n[ui]\nagent_sort = \"{TOKEN}\"\n\
             pr_banner_position = \"{TOKEN}\"\nterminal_font_family = \"{TOKEN};\"\n\
             [capabilities]\nterminal_identity = \"{TOKEN}\"\nclipboard_passthrough = \"{TOKEN}\"\n\
             [editor]\ndefault = \"{TOKEN}\"\n[server]\ncolor = \"{TOKEN}\"\n\
             favicon = \"{TOKEN}\"\ntitle = \"{TOKEN}\\u0007\"\n\
             [providers.claude]\nweb_dragdrop_paste = \"{TOKEN}\"\n"
        );
        let found = corrections_of(&raw);
        assert!(found.len() >= 10, "{found:?}");
        for correction in found {
            assert!(!correction.reason.contains(TOKEN), "{correction:?}");
        }
    }

    // The browser used to apply these rules itself; it runs what it is sent now.

    #[test]
    fn a_zero_timing_means_its_default() {
        assert_eq!(
            effective_heartbeat_seconds(0),
            crate::config::DEFAULT_HEARTBEAT_SECONDS
        );
        assert_eq!(
            effective_reconnect_backoff_cap_seconds(0),
            crate::config::DEFAULT_RECONNECT_BACKOFF_CAP_SECONDS
        );
        assert_eq!(
            effective_reconnect_attempt_timeout_seconds(0),
            crate::config::DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS
        );
        assert_eq!(
            effective_changes_request_timeout_seconds(0),
            crate::config::DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS
        );
        assert_eq!(
            effective_pty_send_timeout_seconds(0),
            crate::config::DEFAULT_PTY_SEND_TIMEOUT_SECONDS
        );
        assert_eq!(effective_heartbeat_seconds(7), 7);
    }

    #[test]
    fn the_changes_request_deadline_caps_at_ten_minutes() {
        assert_eq!(effective_changes_request_timeout_seconds(86_400), 600);
        assert_eq!(effective_changes_request_timeout_seconds(12), 12);
    }

    #[test]
    fn a_heartbeat_deadline_at_or_below_the_period_is_twice_the_period() {
        assert_eq!(effective_heartbeat_deadline_seconds(5, 30), 60);
        assert_eq!(effective_heartbeat_deadline_seconds(20, 20), 40);
        assert_eq!(effective_heartbeat_deadline_seconds(45, 10), 45);
        assert_eq!(effective_heartbeat_deadline_seconds(6, 5), 6);
        // Zero on either side means its default first.
        assert_eq!(effective_heartbeat_deadline_seconds(0, 0), 30);
        assert_eq!(effective_heartbeat_deadline_seconds(0, 40), 80);
    }

    #[test]
    fn the_bootstrap_sends_every_timing_as_dux_uses_it() {
        let raw = "[server]\nheartbeat_seconds = 0\nheartbeat_deadline_seconds = 5\n\
                   changes_request_timeout_seconds = 0\n";
        assert_eq!(
            used_at(raw, &["server", "heartbeat_seconds"]),
            Some(serde_json::json!(15))
        );
        assert_eq!(
            used_at(raw, &["server", "heartbeat_deadline_seconds"]),
            Some(serde_json::json!(30))
        );
        assert_eq!(
            used_at(raw, &["server", "changes_request_timeout_seconds"]),
            Some(serde_json::json!(30))
        );
    }

    #[test]
    fn pr_banner_position_parse_accepts_known_values() {
        assert_eq!(PrBannerPosition::parse("top"), Some(PrBannerPosition::Top));
        assert_eq!(
            PrBannerPosition::parse("  Bottom "),
            Some(PrBannerPosition::Bottom)
        );
    }

    #[test]
    fn pr_banner_position_parse_rejects_unknown() {
        assert_eq!(PrBannerPosition::parse("left"), None);
        assert_eq!(PrBannerPosition::parse(""), None);
    }

    /// CROSS-LANGUAGE PIN: the curated favicon color names live twice, here in
    /// `CURATED_FAVICON_COLORS` and in the TS `FAVICON_COLORS` map that drives the
    /// customize-webapp dialog and the tinted-duck SVG. A recolor/rename dialog that
    /// offered a color the server rejects (or vice versa) would degrade silently, so
    /// this parses the TS map's keys out of `favicon.ts` and asserts the two sets
    /// are identical. Skips (rather than fails) when the web tree isn't present, e.g.
    /// a published crate build outside the workspace. Copies the file-reading and
    /// relative-path approach from `palette::tests::web_pin_matches_the_typescript_pin`.
    #[test]
    fn curated_favicon_colors_match_the_typescript_list() {
        let ts_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../dux-web/web/src/lib/favicon.ts");
        let Ok(source) = std::fs::read_to_string(&ts_path) else {
            eprintln!("skipping: {} not present", ts_path.display());
            return;
        };
        // Isolate the `FAVICON_COLORS` object body, then read the `name: "#hex",`
        // key off each line (the identifier before the first colon).
        let body = source
            .split("export const FAVICON_COLORS: Record<string, string> = {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("FAVICON_COLORS object not found in favicon.ts");
        let mut ts_names: Vec<String> = body
            .lines()
            .filter_map(|line| line.trim().split(':').next())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        ts_names.sort();
        let mut rust_names: Vec<String> = CURATED_FAVICON_COLORS
            .iter()
            .map(|s| s.to_string())
            .collect();
        rust_names.sort();
        assert_eq!(
            rust_names, ts_names,
            "the curated favicon colors drifted between Rust CURATED_FAVICON_COLORS \
             (config_effective.rs) and the TS FAVICON_COLORS map (favicon.ts). Update BOTH lists \
             together so the dialog and the server agree on the accepted colors."
        );
    }

    #[test]
    fn parse_server_favicon_accepts_curated_names() {
        assert_eq!(parse_server_favicon("amber"), Some("amber".into()));
        // Trimmed and lowercased.
        assert_eq!(parse_server_favicon("  VIOLET  "), Some("violet".into()));
        // Every curated name round-trips.
        for name in CURATED_FAVICON_COLORS {
            assert_eq!(parse_server_favicon(name), Some((*name).to_string()));
        }
    }

    #[test]
    fn parse_server_favicon_empty_resets_to_default() {
        assert_eq!(parse_server_favicon(""), Some(String::new()));
        assert_eq!(parse_server_favicon("   "), Some(String::new()));
    }

    #[test]
    fn parse_server_favicon_rejects_unknown() {
        // Dropped legacy names, hex, and URLs are all invalid now. `yellow` is the
        // default (empty), not a tint, so it is rejected as an explicit value.
        assert_eq!(parse_server_favicon("mauve"), None);
        assert_eq!(parse_server_favicon("yellow"), None);
        assert_eq!(parse_server_favicon("purple"), None);
        assert_eq!(parse_server_favicon("#863bff"), None);
        assert_eq!(parse_server_favicon("https://x/y.png"), None);
    }

    #[test]
    fn effective_server_title_strips_controls_and_collapses_whitespace() {
        assert_eq!(effective_server_title("  dux\t\n  prod \r\n "), "dux prod");
        // A bare control character becomes nothing meaningful → default.
        assert_eq!(effective_server_title("\u{0007}\u{0000}"), "dux");
        // Bidi/format characters (Cf) are neutralized too, not just controls (Cc),
        // so a right-to-left override can't spoof the rendered title.
        assert_eq!(
            effective_server_title("invoice\u{202E}cod.exe"),
            "invoice cod.exe"
        );
        assert_eq!(effective_server_title("a\u{200D}\u{FEFF}b"), "a b");
    }

    #[test]
    fn effective_server_title_empty_resets_to_dux() {
        assert_eq!(effective_server_title(""), "dux");
        assert_eq!(effective_server_title("     "), "dux");
    }

    #[test]
    fn effective_server_title_caps_length_by_chars_not_bytes() {
        // Multi-byte glyphs: 300 of them must cap to 200 chars, never panic on a
        // byte boundary inside a codepoint.
        let input: String = "é".repeat(300);
        let out = effective_server_title(&input);
        assert_eq!(out.chars().count(), 200);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn effective_terminal_font_family_keeps_only_the_safe_class() {
        assert_eq!(
            effective_terminal_font_family("Fira Code\n; color: red"),
            "Fira Code color red"
        );
        assert_eq!(effective_terminal_font_family("\u{0007}\u{0000}"), "");
        assert_eq!(effective_terminal_font_family("a\tb\rc"), "abc");
        // Non-ASCII names degrade to the bundled fonts.
        assert_eq!(effective_terminal_font_family("Ünïcödé"), "ncd");
        assert_eq!(effective_terminal_font_family(" é Fira "), "Fira");
    }

    #[test]
    fn effective_terminal_font_family_leaves_ordinary_values_untouched() {
        assert_eq!(effective_terminal_font_family("Fira Code"), "Fira Code");
        assert_eq!(
            effective_terminal_font_family("\"Cascadia Code\", Consolas"),
            "\"Cascadia Code\", Consolas"
        );
        assert_eq!(effective_terminal_font_family(""), "");
    }

    /// The value the browser puts ahead of its bundled fonts is this
    /// function's answer, run unchanged: the TypeScript half of this pin
    /// checks that, and this half checks the answers.
    #[test]
    fn the_shared_terminal_font_family_fixture_holds() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/terminal_font_family_cross_language.json");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let parsed: serde_json::Value = serde_json::from_str(&source).expect("valid JSON");
        let cases = parsed["cases"].as_array().expect("a cases array");
        assert!(cases.len() >= 10, "the fixture has lost its cases");
        for case in cases {
            let configured = case["configured"].as_str().expect("configured");
            let used = case["used"].as_str().expect("used");
            assert_eq!(
                effective_terminal_font_family(configured),
                used,
                "{}",
                case["what"]
            );
            // What dux uses is a fixed point, so a value the Preferences dialog
            // wrote back reads as itself.
            assert_eq!(effective_terminal_font_family(used), used);
        }
    }

    #[test]
    fn effective_terminal_font_family_caps_length_by_chars_not_bytes() {
        let input: String = "a".repeat(300);
        let out = effective_terminal_font_family(&input);
        assert_eq!(out.chars().count(), MAX_TERMINAL_FONT_FAMILY_CHARS);
    }
}
