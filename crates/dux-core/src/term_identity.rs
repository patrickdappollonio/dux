//! Pure resolution of the terminal identity dux presents to an embedded agent.
//!
//! dux runs each agent CLI inside a PTY and, by default, inherits its own
//! environment wholesale. That is a problem: agents detect which terminal they
//! run in from environment variables (`TERM_PROGRAM`, `KITTY_WINDOW_ID`, and so
//! on), and several of them only enable their richer notification channels for a
//! terminal they recognize. Under a kitty+tmux setup, for example, the agent sees
//! `TERM_PROGRAM=tmux` and emits nothing, which is exactly why the attention
//! feature saw no signal in that configuration.
//!
//! This module computes, as a pure function of a small [`HostEnvProbe`] snapshot,
//! the set of environment variables dux should add or remove before spawning an
//! agent so the agent sees a useful, honest terminal identity. It is deliberately
//! free of any PTY, engine, or `std::env` dependency (the probe is built once from
//! `std::env` at engine construction) so it can be exhaustively unit-tested.
//!
//! Two surfaces exist:
//!
//! - **TUI** (`SurfaceKind::Tui`): mirror the real host terminal, seeing through
//!   tmux. If dux is not under tmux, change nothing (the inherited env is already
//!   the real terminal). If dux is under tmux, probe the inherited env for the
//!   OUTER terminal's marker and present that instead, and strip `TMUX`/`TMUX_PANE`
//!   so the agent emits unwrapped escape sequences (dux re-wraps them itself when
//!   forwarding, see `attention::tmux_wrap`).
//! - **Web / headless server** (`SurfaceKind::WebHeadless`): there is no host
//!   terminal to mirror, so force a concrete identity (ghostty by default) that
//!   the browser terminal can honor.

use serde::{Deserialize, Serialize};

/// How dux resolves the terminal identity presented to an agent. Stored in config
/// as a lowercase string (see [`TerminalIdentityMode::from_config_str`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalIdentityMode {
    /// Mirror on the TUI, ghostty on the headless server. The default.
    #[default]
    Auto,
    /// Mirror the real host terminal, seeing through tmux.
    Mirror,
    /// Force a ghostty identity.
    Ghostty,
    /// Force a kitty identity.
    Kitty,
    /// Force an iTerm2 identity.
    Iterm2,
    /// Change nothing: the agent inherits dux's own environment verbatim (the
    /// pre-capabilities behavior, byte for byte).
    None,
}

impl TerminalIdentityMode {
    /// Parse a config string into a mode, returning `None` for an unrecognized
    /// value so the caller can warn and fall back.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "mirror" => Some(Self::Mirror),
            "ghostty" => Some(Self::Ghostty),
            "kitty" => Some(Self::Kitty),
            "iterm2" => Some(Self::Iterm2),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// Parse a config string, falling back to [`TerminalIdentityMode::Auto`] with a
    /// logged warning on an unrecognized value (the theme/color-config convention:
    /// a typo degrades gracefully rather than failing the whole config load).
    pub fn from_config_str(s: &str) -> Self {
        match Self::parse(s) {
            Some(mode) => mode,
            None => {
                crate::logger::warn(&format!(
                    "unknown capabilities.terminal_identity value {s:?}; falling back to \"auto\""
                ));
                Self::Auto
            }
        }
    }
}

/// Which surface is spawning the agent. Decides how [`TerminalIdentityMode::Auto`]
/// resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    /// The terminal UI: a real host terminal exists to mirror.
    Tui,
    /// The headless web server: no host terminal, force an identity.
    WebHeadless,
}

/// The environment mutation dux applies to an agent's spawn. `remove` entries are
/// applied first (a trailing `*` marks a prefix family, e.g. `KITTY_*`), then
/// `set` entries, then the user's own `[env]` overrides last (so an explicit user
/// value always wins).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalIdentity {
    pub set: Vec<(String, String)>,
    pub remove: Vec<String>,
}

impl TerminalIdentity {
    fn empty() -> Self {
        Self::default()
    }
}

/// A snapshot of the identity-relevant environment variables dux inherited, built
/// once from `std::env` at engine construction. Keeping the resolver a pure
/// function of this struct (rather than reading `std::env` directly) is what makes
/// the see-through matrix unit-testable.
#[derive(Clone, Debug, Default)]
pub struct HostEnvProbe {
    /// `TMUX` is set and non-empty: dux itself is running inside tmux.
    pub tmux: bool,
    /// The inherited `TERM_PROGRAM` value, if any.
    pub term_program: Option<String>,
    /// The inherited `TERM_PROGRAM_VERSION` value, if any.
    pub term_program_version: Option<String>,
    /// `GHOSTTY_RESOURCES_DIR` is set: the outer terminal is ghostty.
    pub ghostty_resources_dir: bool,
    /// `KITTY_WINDOW_ID` is set: the outer terminal is kitty.
    pub kitty_window_id: bool,
    /// The inherited `LC_TERMINAL` value, if any (`iTerm2` for iTerm).
    pub lc_terminal: Option<String>,
    /// `ITERM_SESSION_ID` is set: the outer terminal is iTerm2.
    pub iterm_session_id: bool,
}

impl HostEnvProbe {
    /// Build the probe from the current process environment. This is the only
    /// `std::env` read; everything downstream is pure.
    pub fn from_env() -> Self {
        let non_empty = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
        let present = |key: &str| std::env::var_os(key).is_some_and(|v| !v.is_empty());
        Self {
            tmux: present("TMUX"),
            term_program: non_empty("TERM_PROGRAM"),
            term_program_version: non_empty("TERM_PROGRAM_VERSION"),
            ghostty_resources_dir: present("GHOSTTY_RESOURCES_DIR"),
            kitty_window_id: present("KITTY_WINDOW_ID"),
            lc_terminal: non_empty("LC_TERMINAL"),
            iterm_session_id: present("ITERM_SESSION_ID"),
        }
    }

    /// Whether dux is running under tmux: either `TMUX` is set (non-empty) or the
    /// inherited `TERM_PROGRAM` reports tmux. `pub(crate)` so the engine can expose
    /// it as the single tmux predicate (`Engine::host_under_tmux`) for both the
    /// identity resolver and the TUI's passthrough wrap decision.
    pub(crate) fn under_tmux(&self) -> bool {
        self.tmux
            || self
                .term_program
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case("tmux"))
    }
}

/// A pinned ghostty version string presented in forced ghostty mode. Agents key on
/// `TERM_PROGRAM` rather than the version, so the exact value is cosmetic; it is
/// pinned so the identity is stable and reviewable.
pub const GHOSTTY_VERSION: &str = "1.1.3";
/// A pinned iTerm2 version presented in forced iterm2 mode. Cosmetic, see
/// [`GHOSTTY_VERSION`].
pub const ITERM2_VERSION: &str = "3.5.0";

/// Environment variables scrubbed before a FORCED identity is applied so the agent
/// sees only the forced terminal and no leftover markers from dux's real host. A
/// trailing `*` marks a prefix family (every variable whose name starts with the
/// prefix). The scrub is applied by the spawn path, which expands the prefixes
/// against the real environment.
pub const IDENTITY_SCRUB_VARS: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TMUX",
    "TMUX_PANE",
    "KITTY_*",
    "GHOSTTY_*",
    "ITERM_*",
    "LC_TERMINAL*",
    "WEZTERM_*",
    "ALACRITTY_*",
    "KONSOLE_VERSION",
    "VTE_VERSION",
    "WT_SESSION",
    "STY",
    "TERMINAL_EMULATOR",
];

/// Resolve the terminal identity to apply for a spawn. Pure: a function of the
/// requested mode, the spawning surface, and the inherited-env probe.
pub fn resolve_identity(
    mode: TerminalIdentityMode,
    surface: SurfaceKind,
    host_env: &HostEnvProbe,
) -> TerminalIdentity {
    let effective = match mode {
        TerminalIdentityMode::Auto => match surface {
            SurfaceKind::Tui => TerminalIdentityMode::Mirror,
            SurfaceKind::WebHeadless => TerminalIdentityMode::Ghostty,
        },
        other => other,
    };

    match effective {
        // Resolved above; `Auto` cannot reach here.
        TerminalIdentityMode::Auto => TerminalIdentity::empty(),
        TerminalIdentityMode::None => TerminalIdentity::empty(),
        TerminalIdentityMode::Mirror => resolve_mirror(host_env),
        TerminalIdentityMode::Ghostty => forced_ghostty(host_env),
        TerminalIdentityMode::Kitty => forced_kitty(),
        TerminalIdentityMode::Iterm2 => forced_iterm2(),
    }
}

/// Mirror mode: change nothing unless dux runs under tmux, in which case present
/// the OUTER terminal's identity and strip the tmux markers.
fn resolve_mirror(host_env: &HostEnvProbe) -> TerminalIdentity {
    if !host_env.under_tmux() {
        // Not under tmux: the inherited env already IS the real terminal.
        return TerminalIdentity::empty();
    }

    // See through tmux to the outer terminal, in priority order.
    let mut set: Vec<(String, String)> = Vec::new();
    if host_env.ghostty_resources_dir {
        set.push(("TERM_PROGRAM".to_string(), "ghostty".to_string()));
        // The only version var visible under tmux is tmux's own, which would be
        // wrong to present as ghostty's, so it is deliberately omitted. Agents key
        // on TERM_PROGRAM, not the version, so this loses nothing.
    } else if host_env.kitty_window_id {
        // Claude's notification switch matches TERM_PROGRAM verbatim; kitty's own
        // KITTY_* vars already leak through untouched, and we deliberately do NOT
        // set TERM=xterm-kitty (no terminfo risk, and dux's emulator answers the
        // agent's queries, not kitty).
        set.push(("TERM_PROGRAM".to_string(), "kitty".to_string()));
    } else if host_env
        .lc_terminal
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("iTerm2"))
        || host_env.iterm_session_id
    {
        set.push(("TERM_PROGRAM".to_string(), "iTerm.app".to_string()));
        set.push(("LC_TERMINAL".to_string(), "iTerm2".to_string()));
    } else {
        // No recognizable outer marker: honest tmux. Leave the env as-is (agents
        // keep emitting tmux-wrapped sequences, which dux forwards to its own tmux
        // host as-is via re-wrapping).
        return TerminalIdentity::empty();
    }

    // In every see-through case, strip the tmux markers so the agent emits
    // UNWRAPPED sequences; dux re-wraps on forward.
    TerminalIdentity {
        set,
        remove: vec!["TMUX".to_string(), "TMUX_PANE".to_string()],
    }
}

fn scrub_list() -> Vec<String> {
    IDENTITY_SCRUB_VARS.iter().map(|s| s.to_string()).collect()
}

fn forced_ghostty(host_env: &HostEnvProbe) -> TerminalIdentity {
    let mut set = vec![("TERM_PROGRAM".to_string(), "ghostty".to_string())];
    // Present a version: reuse the host's if it looks like a ghostty version,
    // otherwise the pinned constant. Cosmetic either way.
    let version = host_env
        .term_program_version
        .clone()
        .filter(|_| host_env.ghostty_resources_dir)
        .unwrap_or_else(|| GHOSTTY_VERSION.to_string());
    set.push(("TERM_PROGRAM_VERSION".to_string(), version));
    TerminalIdentity {
        set,
        remove: scrub_list(),
    }
}

fn forced_kitty() -> TerminalIdentity {
    TerminalIdentity {
        set: vec![
            ("TERM".to_string(), "xterm-kitty".to_string()),
            ("KITTY_WINDOW_ID".to_string(), "1".to_string()),
        ],
        remove: scrub_list(),
    }
}

fn forced_iterm2() -> TerminalIdentity {
    TerminalIdentity {
        set: vec![
            ("TERM_PROGRAM".to_string(), "iTerm.app".to_string()),
            (
                "TERM_PROGRAM_VERSION".to_string(),
                ITERM2_VERSION.to_string(),
            ),
            ("LC_TERMINAL".to_string(), "iTerm2".to_string()),
            (
                "LC_TERMINAL_VERSION".to_string(),
                ITERM2_VERSION.to_string(),
            ),
        ],
        remove: scrub_list(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_set(id: &TerminalIdentity, key: &str, value: &str) -> bool {
        id.set.iter().any(|(k, v)| k == key && v == value)
    }

    #[test]
    fn auto_resolves_per_surface() {
        // TUI auto == mirror; no tmux -> empty.
        let probe = HostEnvProbe::default();
        let tui = resolve_identity(TerminalIdentityMode::Auto, SurfaceKind::Tui, &probe);
        assert_eq!(tui, TerminalIdentity::empty());

        // Headless auto == ghostty.
        let web = resolve_identity(TerminalIdentityMode::Auto, SurfaceKind::WebHeadless, &probe);
        assert!(has_set(&web, "TERM_PROGRAM", "ghostty"));
        assert!(web.remove.contains(&"TMUX".to_string()));
    }

    #[test]
    fn none_is_empty_on_both_surfaces() {
        let probe = HostEnvProbe {
            tmux: true,
            kitty_window_id: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_identity(TerminalIdentityMode::None, SurfaceKind::Tui, &probe),
            TerminalIdentity::empty()
        );
        assert_eq!(
            resolve_identity(TerminalIdentityMode::None, SurfaceKind::WebHeadless, &probe),
            TerminalIdentity::empty()
        );
    }

    #[test]
    fn mirror_empty_without_tmux() {
        let probe = HostEnvProbe {
            kitty_window_id: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_identity(TerminalIdentityMode::Mirror, SurfaceKind::Tui, &probe),
            TerminalIdentity::empty()
        );
    }

    #[test]
    fn mirror_sees_through_tmux_to_kitty() {
        let probe = HostEnvProbe {
            tmux: true,
            term_program: Some("tmux".to_string()),
            kitty_window_id: true,
            ..Default::default()
        };
        let id = resolve_identity(TerminalIdentityMode::Mirror, SurfaceKind::Tui, &probe);
        assert!(has_set(&id, "TERM_PROGRAM", "kitty"));
        assert!(id.remove.contains(&"TMUX".to_string()));
        assert!(id.remove.contains(&"TMUX_PANE".to_string()));
        // Deliberately does not pin TERM.
        assert!(!id.set.iter().any(|(k, _)| k == "TERM"));
    }

    #[test]
    fn mirror_sees_through_tmux_to_ghostty() {
        let probe = HostEnvProbe {
            tmux: true,
            ghostty_resources_dir: true,
            ..Default::default()
        };
        let id = resolve_identity(TerminalIdentityMode::Mirror, SurfaceKind::Tui, &probe);
        assert!(has_set(&id, "TERM_PROGRAM", "ghostty"));
        assert!(id.remove.contains(&"TMUX".to_string()));
    }

    #[test]
    fn mirror_sees_through_tmux_to_iterm() {
        let probe = HostEnvProbe {
            tmux: true,
            lc_terminal: Some("iTerm2".to_string()),
            ..Default::default()
        };
        let id = resolve_identity(TerminalIdentityMode::Mirror, SurfaceKind::Tui, &probe);
        assert!(has_set(&id, "TERM_PROGRAM", "iTerm.app"));
        assert!(has_set(&id, "LC_TERMINAL", "iTerm2"));
        assert!(id.remove.contains(&"TMUX".to_string()));
    }

    #[test]
    fn mirror_honest_tmux_when_no_marker() {
        // Under tmux but no recognizable outer terminal: leave env as-is, keep TMUX.
        let probe = HostEnvProbe {
            tmux: true,
            ..Default::default()
        };
        let id = resolve_identity(TerminalIdentityMode::Mirror, SurfaceKind::Tui, &probe);
        assert_eq!(id, TerminalIdentity::empty());
    }

    #[test]
    fn ghostty_forced_scrubs_then_sets() {
        let probe = HostEnvProbe::default();
        let id = resolve_identity(TerminalIdentityMode::Ghostty, SurfaceKind::Tui, &probe);
        assert!(has_set(&id, "TERM_PROGRAM", "ghostty"));
        assert!(has_set(&id, "TERM_PROGRAM_VERSION", GHOSTTY_VERSION));
        assert!(id.remove.contains(&"TMUX".to_string()));
        assert!(id.remove.contains(&"KITTY_*".to_string()));
    }

    #[test]
    fn kitty_forced_sets_term_and_window_id() {
        let id = resolve_identity(
            TerminalIdentityMode::Kitty,
            SurfaceKind::WebHeadless,
            &HostEnvProbe::default(),
        );
        assert!(has_set(&id, "TERM", "xterm-kitty"));
        assert!(has_set(&id, "KITTY_WINDOW_ID", "1"));
        assert!(id.remove.contains(&"GHOSTTY_*".to_string()));
    }

    #[test]
    fn iterm2_forced_sets_full_identity() {
        let id = resolve_identity(
            TerminalIdentityMode::Iterm2,
            SurfaceKind::Tui,
            &HostEnvProbe::default(),
        );
        assert!(has_set(&id, "TERM_PROGRAM", "iTerm.app"));
        assert!(has_set(&id, "TERM_PROGRAM_VERSION", ITERM2_VERSION));
        assert!(has_set(&id, "LC_TERMINAL", "iTerm2"));
        assert!(has_set(&id, "LC_TERMINAL_VERSION", ITERM2_VERSION));
    }

    #[test]
    fn scrub_list_contains_tmux() {
        assert!(IDENTITY_SCRUB_VARS.contains(&"TMUX"));
    }

    #[test]
    fn mode_parse_roundtrip() {
        assert_eq!(
            TerminalIdentityMode::parse("MIRROR"),
            Some(TerminalIdentityMode::Mirror)
        );
        assert_eq!(TerminalIdentityMode::parse("nope"), None);
        assert_eq!(
            TerminalIdentityMode::from_config_str("nonsense"),
            TerminalIdentityMode::Auto
        );
    }
}
