#![allow(dead_code)]

use std::path::Path;

use anyhow::{Context, Result};
use opaline::{OpalineColor, Theme as OpalineTheme};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::config::DuxPaths;

/// Braille dot-pattern frames for spinner animations. Shared by the loading
/// card, status line, and left-pane streaming indicator.
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Name of the bundled default theme — also the value that ships in the
/// generated `config.toml` on first boot.
pub const DEFAULT_THEME_NAME: &str = "dux_dark";

/// The bundled `dux_dark` theme TOML, embedded at compile time so the default
/// path never depends on a file on disk.
const DUX_DARK_TOML: &str = include_str!("../assets/themes/dux_dark.toml");

const GITHUB_PR_OPEN_BG: OpalineColor = OpalineColor::new(35, 134, 54);
const GITHUB_PR_MERGED_BG: OpalineColor = OpalineColor::new(130, 80, 223);
const GITHUB_PR_CLOSED_BG: OpalineColor = OpalineColor::new(110, 54, 48);
const GITHUB_PR_OPEN_LABEL: OpalineColor = OpalineColor::new(0, 255, 0);
const GITHUB_PR_MERGED_LABEL: OpalineColor = OpalineColor::new(170, 100, 220);
const GITHUB_PR_CLOSED_LABEL: OpalineColor = OpalineColor::new(140, 80, 80);

pub struct Theme {
    /// Base surface color for the dux app — used as a frame-wide pre-fill so
    /// every cell that no widget explicitly paints (gutters, modal interiors,
    /// the row behind the PR pill caps, etc.) inherits the active theme's
    /// background instead of the user's terminal default.
    pub app_bg: Color,
    /// Primary body-text color used by widgets that render plain bold or
    /// emphasis text on top of `app_bg` (project names, "Current: …" lines
    /// in modals, etc.). Without an explicit fg these spans fall through to
    /// the terminal's default foreground, which becomes invisible on light
    /// themes — this field gives them a theme-driven color that contrasts
    /// with `app_bg`.
    pub text_fg: Color,
    pub header_fg: Color,
    pub header_bg: Color,
    pub header_label_fg: Color,
    pub header_separator_fg: Color,
    pub border_focused: Color,
    pub border_normal: Color,
    pub title_focused: Color,
    pub title_normal: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
    pub project_icon: Color,
    pub project_missing_fg: Color,
    pub session_active: Color,
    pub session_detached: Color,
    pub session_exited: Color,
    /// Foreground used for a session row whose worktree is currently being
    /// removed by a background worker. Defaults to the same shade as
    /// `session_exited`; the render code adds `Modifier::ITALIC` to
    /// visually distinguish the two states. A separate theme slot so that
    /// future theme customization can differentiate the colors without a
    /// code change.
    pub session_deleting: Color,
    pub status_info_fg: Color,
    pub status_info_bg: Color,
    pub status_busy_fg: Color,
    pub status_busy_bg: Color,
    pub status_error_fg: Color,
    pub status_error_bg: Color,
    pub diff_add: Color,
    pub diff_remove: Color,
    pub diff_hunk: Color,
    pub diff_file_header: Color,
    pub file_status_fg: Color,
    pub hint_key_fg: Color,
    pub hint_bracket_fg: Color,
    pub hint_key_bg: Color,
    pub hint_desc_fg: Color,
    pub hint_dim_key_fg: Color,
    pub hint_dim_bracket_fg: Color,
    pub hint_dim_desc_fg: Color,
    pub hint_bar_bg: Color,
    pub overlay_border: Color,
    pub overlay_bg: Color,
    pub overlay_dim_bg: Color,
    pub prompt_cursor: Color,
    pub provider_label_fg: Color,
    pub branch_fg: Color,
    pub terminal_hint_fg: Color,
    pub scroll_indicator_fg: Color,
    pub scroll_indicator_bg: Color,
    pub warning_fg: Color,
    pub button_active_fg: Color,
    pub button_confirm_border: Color,
    pub button_danger_border: Color,
    pub overlay_dim_fg: Color,
    pub diff_add_bg: Color,
    pub diff_remove_bg: Color,
    pub help_section_header_fg: Color,
    pub input_cursor_fg: Color,
    pub input_cursor_bg: Color,
    pub input_label_fg: Color,
    pub diff_binary_fg: Color,
    pub diff_stat_add_fg: Color,
    pub diff_stat_remove_fg: Color,
    pub runtime_context_value_fg: Color,
    pub nudge_border: Color,
    pub tip_pill_fg: Color,
    pub tip_pill_bg: Color,
    pub tip_text_fg: Color,
    pub tip_highlight_fg: Color,
    pub diff_line_number_fg: Color,
    pub diff_line_number_sep: Color,
    pub pr_open_bg: Color,
    pub pr_merged_bg: Color,
    pub pr_closed_bg: Color,
    pub pr_banner_fg: Color,
    pub pr_merged_label: Color,
    pub pr_closed_label: Color,
    pub pr_open_label: Color,
    pub help_banner_fg: Color,
    pub help_banner_bg: Color,
    pub help_body_fg: Color,
}

/// Load a theme by name.
///
/// Resolution order:
///  1. `<config_dir>/themes/<name>.toml` — user-authored themes win first.
///  2. The bundled `dux-dark` TOML (embedded at compile time).
///  3. An Opaline built-in (Catppuccin, Nord, Dracula, …). Names use
///     underscores, e.g. `tokyo_night`, `catppuccin_mocha`.
///
/// Returns an error if none of the above match. The caller is expected to fall
/// back to [`Theme::fallback`] in that case.
pub fn load(name: &str, paths: &DuxPaths) -> Result<Theme> {
    let user_path = paths.root.join("themes").join(format!("{name}.toml"));
    if user_path.exists() {
        return load_from_file(&user_path)
            .with_context(|| format!("failed to load user theme {}", user_path.display()));
    }

    if name == DEFAULT_THEME_NAME {
        return load_from_str(DUX_DARK_TOML)
            .context("bundled dux-dark theme failed to load (this is a dux bug)");
    }

    // Theme ids in dux match opaline's TOML filenames (`catppuccin_mocha`,
    // `tokyo_night`). Opaline's runtime registry uses kebab-case ids derived
    // from those filenames, so translate underscores → hyphens before the
    // lookup. The original form is also tried as a fallback to forgive any
    // legacy hyphenated id that may already live in someone's config.
    let candidates: [String; 2] = [name.replace('_', "-"), name.to_string()];
    for candidate in candidates.iter() {
        if let Some(mut theme) = opaline::load_by_name(candidate) {
            register_dux_defaults(&mut theme);
            return Ok(Theme::from_opaline(&theme));
        }
    }

    Err(anyhow::anyhow!(
        "unknown theme '{name}' — try '{DEFAULT_THEME_NAME}', a built-in name like \
         'catppuccin_mocha' / 'nord' / 'tokyo_night', or place a TOML file at \
         {}/themes/<name>.toml",
        paths.root.display()
    ))
}

/// Where a theme came from — used by the theme picker to label entries and
/// disambiguate same-named user themes from built-ins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// The bundled `dux-dark` theme — always present, always first in the list.
    Bundled,
    /// A theme compiled into the opaline crate (e.g. `nord`, `catppuccin-mocha`).
    Opaline,
    /// A user-authored theme found at `<config_dir>/themes/<name>.toml`.
    User,
}

/// Metadata about an available theme, used to populate the theme picker.
#[derive(Clone, Debug)]
pub struct ThemeListing {
    /// Identifier passed to [`load`] — file stem for user themes, kebab-case
    /// id for built-ins, `dux-dark` for the bundled default.
    pub id: String,
    /// Human-readable label shown in the picker.
    pub display_name: String,
    pub source: ThemeSource,
}

/// Enumerate every theme reachable from the dux runtime: the bundled
/// `dux-dark`, the opaline built-ins, and any TOML files the user has
/// dropped in `<config_dir>/themes/`. Sorted with `dux-dark` first, then
/// user themes, then built-ins alphabetically — predictable scrolling.
pub fn discover_available(paths: &DuxPaths) -> Vec<ThemeListing> {
    let mut themes = Vec::new();

    themes.push(ThemeListing {
        id: DEFAULT_THEME_NAME.to_string(),
        display_name: format!("{DEFAULT_THEME_NAME} (bundled default)"),
        source: ThemeSource::Bundled,
    });

    let user_dir = paths.root.join("themes");
    if let Ok(entries) = std::fs::read_dir(&user_dir) {
        let mut user_themes: Vec<ThemeListing> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "toml") {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?.to_string();
                if stem == DEFAULT_THEME_NAME {
                    // dux-dark stays the bundled entry; a user file with the
                    // same name still loads first via `theme::load`, but we
                    // don't show two "dux-dark" rows in the picker.
                    return None;
                }
                Some(ThemeListing {
                    display_name: format!("{stem} (user)"),
                    id: stem,
                    source: ThemeSource::User,
                })
            })
            .collect();
        user_themes.sort_by(|a, b| a.id.cmp(&b.id));
        themes.extend(user_themes);
    }

    let mut builtin: Vec<ThemeListing> = opaline::list_available_themes()
        .into_iter()
        .filter(|info| info.builtin)
        .map(|info| ThemeListing {
            // Match the opaline TOML filenames (underscored) for the
            // user-facing id; `theme::load` reverses the conversion before
            // calling opaline.
            id: info.name.replace('-', "_"),
            display_name: info.display_name.clone(),
            source: ThemeSource::Opaline,
        })
        .collect();
    builtin.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    themes.extend(builtin);

    themes
}

/// Load a theme by name and fall back to the bundled `dux-dark` if the lookup
/// fails. Returns the theme and an optional human-readable warning message
/// suitable for the status line (`Some` only when a fallback occurred).
pub fn load_or_fallback(name: &str, paths: &DuxPaths) -> (Theme, Option<String>) {
    match load(name, paths) {
        Ok(theme) => (theme, None),
        Err(err) => {
            crate::logger::warn(&format!("theme load failed: {err:#}"));
            (
                Theme::fallback(),
                Some(format!(
                    "Theme '{name}' could not be loaded — falling back to {DEFAULT_THEME_NAME}. \
                     See dux.log for details."
                )),
            )
        }
    }
}

fn load_from_file(path: &Path) -> Result<Theme> {
    let mut theme = opaline::load_from_file(path)
        .with_context(|| format!("opaline failed to parse {}", path.display()))?;
    register_dux_defaults(&mut theme);
    Ok(Theme::from_opaline(&theme))
}

fn load_from_str(toml_str: &str) -> Result<Theme> {
    let mut theme =
        opaline::load_from_str(toml_str, None).context("opaline failed to parse TOML")?;
    register_dux_defaults(&mut theme);
    Ok(Theme::from_opaline(&theme))
}

/// Inject derived `dux.*` tokens into a theme that doesn't already define
/// them. This lets generic Opaline built-ins (Nord, Catppuccin, …) drive the
/// dux UI without having to know the dux token namespace, while still letting
/// dux-aware themes override every dux token explicitly.
fn register_dux_defaults(theme: &mut OpalineTheme) {
    // Resolve the standard Opaline semantic tokens once. `theme.color()`
    // returns OpalineColor::FALLBACK (gray) for missing tokens, which keeps
    // unknown themes legible rather than panicking.
    let text_primary = theme.color("text.primary");
    let text_muted = theme.color("text.muted");
    let text_dim = theme.color("text.dim");
    let bg_base = theme.color("bg.base");
    let bg_panel = theme.color("bg.panel");
    theme.register_default_token("dux.app_bg", bg_base);
    theme.register_default_token("dux.text_fg", text_primary);
    let bg_highlight = theme.color("bg.highlight");
    let bg_active = theme.color("bg.active");
    let accent_primary = theme.color("accent.primary");
    let accent_secondary = theme.color("accent.secondary");
    let border_focused = theme.color("border.focused");
    let border_unfocused = theme.color("border.unfocused");
    let success = theme.color("success");
    let error = theme.color("error");
    let warning = theme.color("warning");
    let info = theme.color("info");

    // Header / chrome
    theme.register_default_token("dux.header_fg", text_primary);
    theme.register_default_token("dux.header_bg", bg_base);
    theme.register_default_token("dux.header_label_fg", text_muted);
    theme.register_default_token("dux.header_separator_fg", border_unfocused);

    // Borders & titles
    theme.register_default_token("dux.border_focused", border_focused);
    theme.register_default_token("dux.border_normal", border_unfocused);
    theme.register_default_token("dux.title_focused", accent_primary);
    theme.register_default_token("dux.title_normal", text_muted);

    // Selection
    theme.register_default_token("dux.selection_fg", bg_base);
    theme.register_default_token("dux.selection_bg", accent_primary);

    // Projects & sessions
    theme.register_default_token("dux.project_icon", accent_primary);
    theme.register_default_token("dux.project_missing_fg", warning);
    theme.register_default_token("dux.session_active", text_primary);
    theme.register_default_token("dux.session_detached", warning);
    theme.register_default_token("dux.session_exited", text_dim);
    theme.register_default_token("dux.session_deleting", text_dim);

    // Status line
    theme.register_default_token("dux.status_info_fg", text_muted);
    theme.register_default_token("dux.status_info_bg", bg_panel);
    theme.register_default_token("dux.status_busy_fg", warning);
    theme.register_default_token("dux.status_busy_bg", bg_panel);
    theme.register_default_token("dux.status_error_fg", error);
    theme.register_default_token("dux.status_error_bg", bg_panel);

    // Diff
    theme.register_default_token("dux.diff_add", success);
    theme.register_default_token("dux.diff_remove", error);
    theme.register_default_token("dux.diff_hunk", accent_secondary);
    theme.register_default_token("dux.diff_file_header", text_primary);
    theme.register_default_token("dux.file_status_fg", warning);
    theme.register_default_token("dux.diff_add_bg", bg_panel);
    theme.register_default_token("dux.diff_remove_bg", bg_panel);
    theme.register_default_token("dux.diff_binary_fg", warning);
    theme.register_default_token("dux.diff_stat_add_fg", success);
    theme.register_default_token("dux.diff_stat_remove_fg", error);
    theme.register_default_token("dux.diff_line_number_fg", text_dim);
    theme.register_default_token("dux.diff_line_number_sep", border_unfocused);

    // Hint / footer bar
    theme.register_default_token("dux.hint_key_fg", accent_primary);
    theme.register_default_token("dux.hint_bracket_fg", text_dim);
    theme.register_default_token("dux.hint_key_bg", bg_panel);
    theme.register_default_token("dux.hint_desc_fg", text_muted);
    theme.register_default_token("dux.hint_dim_key_fg", text_dim);
    theme.register_default_token("dux.hint_dim_bracket_fg", border_unfocused);
    theme.register_default_token("dux.hint_dim_desc_fg", text_dim);
    theme.register_default_token("dux.hint_bar_bg", bg_panel);

    // Overlays
    theme.register_default_token("dux.overlay_border", border_focused);
    theme.register_default_token("dux.overlay_bg", bg_panel);
    theme.register_default_token("dux.overlay_dim_bg", bg_base);
    theme.register_default_token("dux.overlay_dim_fg", text_dim);
    theme.register_default_token("dux.prompt_cursor", accent_primary);

    // Provider / branch / scrollback
    theme.register_default_token("dux.provider_label_fg", text_muted);
    theme.register_default_token("dux.branch_fg", accent_primary);
    theme.register_default_token("dux.terminal_hint_fg", text_dim);
    theme.register_default_token("dux.scroll_indicator_fg", text_primary);
    theme.register_default_token("dux.scroll_indicator_bg", bg_active);
    theme.register_default_token("dux.warning_fg", warning);

    // Buttons
    theme.register_default_token("dux.button_active_fg", text_primary);
    theme.register_default_token("dux.button_confirm_border", accent_primary);
    theme.register_default_token("dux.button_danger_border", error);

    // Help overlay
    theme.register_default_token("dux.help_section_header_fg", accent_primary);
    theme.register_default_token("dux.help_banner_fg", bg_base);
    theme.register_default_token("dux.help_banner_bg", accent_primary);
    theme.register_default_token("dux.help_body_fg", text_muted);

    // Input
    theme.register_default_token("dux.input_cursor_fg", bg_base);
    theme.register_default_token("dux.input_cursor_bg", text_primary);
    theme.register_default_token("dux.input_label_fg", text_primary);

    // Misc UI surfaces
    theme.register_default_token("dux.runtime_context_value_fg", info);
    theme.register_default_token("dux.nudge_border", warning);
    theme.register_default_token("dux.tip_pill_fg", text_primary);
    theme.register_default_token("dux.tip_pill_bg", bg_highlight);
    theme.register_default_token("dux.tip_text_fg", text_dim);
    theme.register_default_token("dux.tip_highlight_fg", accent_secondary);

    // Pull request colors default to GitHub's PR state palette instead of the
    // active theme's semantic colors. User themes may still override these
    // `dux.pr_*` tokens explicitly, but generic themes should not accidentally
    // repaint GitHub-specific open/closed/merged statuses.
    theme.register_default_token("dux.pr_open_bg", GITHUB_PR_OPEN_BG);
    theme.register_default_token("dux.pr_merged_bg", GITHUB_PR_MERGED_BG);
    theme.register_default_token("dux.pr_closed_bg", GITHUB_PR_CLOSED_BG);
    theme.register_default_token("dux.pr_banner_fg", OpalineColor::WHITE);
    theme.register_default_token("dux.pr_open_label", GITHUB_PR_OPEN_LABEL);
    theme.register_default_token("dux.pr_merged_label", GITHUB_PR_MERGED_LABEL);
    theme.register_default_token("dux.pr_closed_label", GITHUB_PR_CLOSED_LABEL);
}

/// Convert an [`OpalineColor`] (always RGB) into a [`ratatui::style::Color`],
/// remapping a few canonical RGB values back to their named ANSI equivalents.
///
/// The remap exists to keep the dux-dark default visually identical to the
/// pre-Opaline palette, which used `Color::Cyan`, `Color::Yellow`, etc.
/// directly. On terminals that honor the user's customized 16-color profile
/// (where "cyan" may not be `#00ffff`), this preserves that behavior. Custom
/// themes that happen to specify exactly `#00ffff` will also benefit from the
/// same terminal-palette routing — a deliberate choice for predictability.
fn into_ratatui(color: OpalineColor) -> Color {
    match (color.r, color.g, color.b) {
        (0, 0, 0) => Color::Black,
        (255, 255, 255) => Color::White,
        (255, 0, 0) => Color::Red,
        (0, 255, 0) => Color::Green,
        (255, 255, 0) => Color::Yellow,
        (0, 0, 255) => Color::Blue,
        (255, 0, 255) => Color::Magenta,
        (0, 255, 255) => Color::Cyan,
        (128, 128, 128) => Color::DarkGray,
        (r, g, b) => Color::Rgb(r, g, b),
    }
}

impl Theme {
    /// Build a [`Theme`] from a fully-resolved Opaline theme. Every field is
    /// looked up via the `dux.<field>` namespace; defaults are injected by
    /// [`register_dux_defaults`] before this is called, so missing tokens fall
    /// back to standard Opaline semantic colors rather than the `FALLBACK`
    /// gray.
    pub fn from_opaline(theme: &OpalineTheme) -> Self {
        let pick = |token: &str| into_ratatui(theme.color(token));
        Self {
            app_bg: pick("dux.app_bg"),
            text_fg: pick("dux.text_fg"),
            header_fg: pick("dux.header_fg"),
            header_bg: pick("dux.header_bg"),
            header_label_fg: pick("dux.header_label_fg"),
            header_separator_fg: pick("dux.header_separator_fg"),
            border_focused: pick("dux.border_focused"),
            border_normal: pick("dux.border_normal"),
            title_focused: pick("dux.title_focused"),
            title_normal: pick("dux.title_normal"),
            selection_fg: pick("dux.selection_fg"),
            selection_bg: pick("dux.selection_bg"),
            project_icon: pick("dux.project_icon"),
            project_missing_fg: pick("dux.project_missing_fg"),
            session_active: pick("dux.session_active"),
            session_detached: pick("dux.session_detached"),
            session_exited: pick("dux.session_exited"),
            session_deleting: pick("dux.session_deleting"),
            status_info_fg: pick("dux.status_info_fg"),
            status_info_bg: pick("dux.status_info_bg"),
            status_busy_fg: pick("dux.status_busy_fg"),
            status_busy_bg: pick("dux.status_busy_bg"),
            status_error_fg: pick("dux.status_error_fg"),
            status_error_bg: pick("dux.status_error_bg"),
            diff_add: pick("dux.diff_add"),
            diff_remove: pick("dux.diff_remove"),
            diff_hunk: pick("dux.diff_hunk"),
            diff_file_header: pick("dux.diff_file_header"),
            file_status_fg: pick("dux.file_status_fg"),
            hint_key_fg: pick("dux.hint_key_fg"),
            hint_bracket_fg: pick("dux.hint_bracket_fg"),
            hint_key_bg: pick("dux.hint_key_bg"),
            hint_desc_fg: pick("dux.hint_desc_fg"),
            hint_dim_key_fg: pick("dux.hint_dim_key_fg"),
            hint_dim_bracket_fg: pick("dux.hint_dim_bracket_fg"),
            hint_dim_desc_fg: pick("dux.hint_dim_desc_fg"),
            hint_bar_bg: pick("dux.hint_bar_bg"),
            overlay_border: pick("dux.overlay_border"),
            overlay_bg: pick("dux.overlay_bg"),
            overlay_dim_bg: pick("dux.overlay_dim_bg"),
            prompt_cursor: pick("dux.prompt_cursor"),
            provider_label_fg: pick("dux.provider_label_fg"),
            branch_fg: pick("dux.branch_fg"),
            terminal_hint_fg: pick("dux.terminal_hint_fg"),
            scroll_indicator_fg: pick("dux.scroll_indicator_fg"),
            scroll_indicator_bg: pick("dux.scroll_indicator_bg"),
            warning_fg: pick("dux.warning_fg"),
            button_active_fg: pick("dux.button_active_fg"),
            button_confirm_border: pick("dux.button_confirm_border"),
            button_danger_border: pick("dux.button_danger_border"),
            overlay_dim_fg: pick("dux.overlay_dim_fg"),
            diff_add_bg: pick("dux.diff_add_bg"),
            diff_remove_bg: pick("dux.diff_remove_bg"),
            help_section_header_fg: pick("dux.help_section_header_fg"),
            input_cursor_fg: pick("dux.input_cursor_fg"),
            input_cursor_bg: pick("dux.input_cursor_bg"),
            input_label_fg: pick("dux.input_label_fg"),
            diff_binary_fg: pick("dux.diff_binary_fg"),
            diff_stat_add_fg: pick("dux.diff_stat_add_fg"),
            diff_stat_remove_fg: pick("dux.diff_stat_remove_fg"),
            runtime_context_value_fg: pick("dux.runtime_context_value_fg"),
            nudge_border: pick("dux.nudge_border"),
            tip_pill_fg: pick("dux.tip_pill_fg"),
            tip_pill_bg: pick("dux.tip_pill_bg"),
            tip_text_fg: pick("dux.tip_text_fg"),
            tip_highlight_fg: pick("dux.tip_highlight_fg"),
            diff_line_number_fg: pick("dux.diff_line_number_fg"),
            diff_line_number_sep: pick("dux.diff_line_number_sep"),
            pr_open_bg: pick("dux.pr_open_bg"),
            pr_merged_bg: pick("dux.pr_merged_bg"),
            pr_closed_bg: pick("dux.pr_closed_bg"),
            pr_banner_fg: pick("dux.pr_banner_fg"),
            pr_merged_label: pick("dux.pr_merged_label"),
            pr_closed_label: pick("dux.pr_closed_label"),
            pr_open_label: pick("dux.pr_open_label"),
            help_banner_fg: pick("dux.help_banner_fg"),
            help_banner_bg: pick("dux.help_banner_bg"),
            help_body_fg: pick("dux.help_body_fg"),
        }
    }

    /// Last-resort theme used when `theme::load` fails. Loads the bundled
    /// `dux-dark.toml` which is embedded at compile time, so this never fails
    /// in a correctly-built binary.
    pub fn fallback() -> Self {
        load_from_str(DUX_DARK_TOML).expect("bundled dux-dark theme must always parse")
    }

    /// The original dux dark palette. Kept for tests, snapshots, and any
    /// future caller that wants the exact pre-Opaline default without
    /// going through the loader. Equivalent to [`Self::fallback`].
    pub fn default_dark() -> Self {
        Self::fallback()
    }

    pub fn header_style(&self) -> Style {
        Style::default().fg(self.header_fg).bg(self.header_bg)
    }

    pub fn border_style(&self, focused: bool) -> Style {
        if focused {
            Style::default().fg(self.border_focused)
        } else {
            Style::default().fg(self.border_normal)
        }
    }

    pub fn title_style(&self, focused: bool) -> Style {
        if focused {
            Style::default()
                .fg(self.title_focused)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.title_normal)
        }
    }

    pub fn selection_style(&self) -> Style {
        Style::default()
            .fg(self.selection_fg)
            .bg(self.selection_bg)
            .add_modifier(Modifier::BOLD)
    }

    pub fn status_style(&self, tone: crate::statusline::StatusTone) -> Style {
        match tone {
            crate::statusline::StatusTone::Info => Style::default()
                .fg(self.status_info_fg)
                .bg(self.status_info_bg),
            crate::statusline::StatusTone::Busy => Style::default()
                .fg(self.status_busy_fg)
                .bg(self.status_busy_bg),
            crate::statusline::StatusTone::Warning => {
                Style::default().fg(self.warning_fg).bg(self.status_info_bg)
            }
            crate::statusline::StatusTone::Error => Style::default()
                .fg(self.status_error_fg)
                .bg(self.status_error_bg),
        }
    }

    pub fn status_dot(&self, tone: crate::statusline::StatusTone) -> (&'static str, Color) {
        match tone {
            crate::statusline::StatusTone::Info => ("●", self.session_active),
            crate::statusline::StatusTone::Busy => ("●", self.session_detached),
            crate::statusline::StatusTone::Warning => ("●", self.warning_fg),
            crate::statusline::StatusTone::Error => ("●", self.status_error_fg),
        }
    }

    pub fn session_dot(&self, status: &crate::model::SessionStatus) -> (&'static str, Color) {
        match status {
            crate::model::SessionStatus::Active => ("●", self.session_active),
            crate::model::SessionStatus::Detached => ("◐", self.session_detached),
            crate::model::SessionStatus::Exited => ("○", self.session_exited),
        }
    }

    /// Render a key badge as `<key>` with the angle brackets in an accent color
    /// and the key name in bold. Returns 3 spans.
    pub fn dim_key_badge<'a>(&self, key: &'a str, bg: Color) -> Vec<Span<'a>> {
        vec![
            Span::styled("<", Style::default().fg(self.hint_dim_bracket_fg).bg(bg)),
            Span::styled(
                key,
                Style::default()
                    .fg(self.hint_dim_key_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(">", Style::default().fg(self.hint_dim_bracket_fg).bg(bg)),
        ]
    }

    pub fn key_badge<'a>(&self, key: &'a str, bg: Color) -> Vec<Span<'a>> {
        vec![
            Span::styled("<", Style::default().fg(self.hint_bracket_fg).bg(bg)),
            Span::styled(
                key,
                Style::default()
                    .fg(self.hint_key_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(">", Style::default().fg(self.hint_bracket_fg).bg(bg)),
        ]
    }

    pub fn key_badge_default<'a>(&self, key: &'a str) -> Vec<Span<'a>> {
        // Pass `app_bg` rather than `Color::Reset` so the badge background
        // tracks the active theme. With `Color::Reset` the bracket and key
        // cells emit an SGR that overrides the surrounding pre-fill and
        // falls through to the user's terminal default — which on a dark
        // terminal kept those badges dark even after switching to a light
        // dux theme.
        self.key_badge(key, self.app_bg)
    }

    pub fn dim_key_badge_default<'a>(&self, key: &'a str) -> Vec<Span<'a>> {
        self.dim_key_badge(key, self.app_bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snapshot of the original `Theme::default_dark()` palette, kept here as
    /// an oracle so the bundled dux-dark TOML can be verified field-for-field
    /// against the pre-Opaline behavior.
    fn default_dark_archived() -> Theme {
        Theme {
            // app_bg matches the historical overlay_bg shade so dark-terminal
            // dux-dark users see no perceptible change after the frame
            // pre-fill landed.
            app_bg: Color::Rgb(20, 20, 20),
            // text_fg matches the historical "default fg" — pure white on
            // dark — so unstyled body text (project names, modal "Current:"
            // labels, etc.) renders identically to before for dux-dark.
            text_fg: Color::White,
            header_fg: Color::White,
            header_bg: Color::Rgb(30, 30, 30),
            header_label_fg: Color::Rgb(120, 120, 120),
            header_separator_fg: Color::Rgb(60, 60, 60),
            border_focused: Color::Cyan,
            border_normal: Color::Rgb(80, 80, 80),
            title_focused: Color::Cyan,
            title_normal: Color::Rgb(140, 140, 140),
            selection_fg: Color::Black,
            selection_bg: Color::Cyan,
            project_icon: Color::Rgb(100, 149, 237),
            project_missing_fg: Color::Rgb(180, 160, 80),
            session_active: Color::Rgb(210, 210, 210),
            session_detached: Color::Yellow,
            session_exited: Color::Rgb(100, 100, 100),
            session_deleting: Color::Rgb(100, 100, 100),
            status_info_fg: Color::Rgb(100, 100, 100),
            status_info_bg: Color::Rgb(25, 25, 25),
            status_busy_fg: Color::Yellow,
            status_busy_bg: Color::Rgb(40, 35, 15),
            status_error_fg: Color::Red,
            status_error_bg: Color::Rgb(50, 20, 20),
            diff_add: Color::Green,
            diff_remove: Color::Red,
            diff_hunk: Color::Magenta,
            diff_file_header: Color::White,
            file_status_fg: Color::Yellow,
            hint_key_fg: Color::Cyan,
            hint_bracket_fg: Color::DarkGray,
            hint_key_bg: Color::Rgb(35, 35, 35),
            hint_desc_fg: Color::Rgb(160, 160, 160),
            hint_dim_key_fg: Color::Rgb(80, 140, 160),
            hint_dim_bracket_fg: Color::Rgb(60, 60, 60),
            hint_dim_desc_fg: Color::Rgb(100, 100, 100),
            hint_bar_bg: Color::Rgb(25, 25, 25),
            overlay_border: Color::Cyan,
            overlay_bg: Color::Rgb(20, 20, 20),
            overlay_dim_bg: Color::Rgb(10, 10, 10),
            prompt_cursor: Color::Cyan,
            provider_label_fg: Color::Rgb(100, 100, 100),
            branch_fg: Color::Cyan,
            terminal_hint_fg: Color::Rgb(80, 80, 80),
            scroll_indicator_fg: Color::Rgb(210, 210, 210),
            scroll_indicator_bg: Color::Rgb(55, 55, 55),
            warning_fg: Color::Yellow,
            button_active_fg: Color::White,
            button_confirm_border: Color::Cyan,
            button_danger_border: Color::Red,
            overlay_dim_fg: Color::DarkGray,
            diff_add_bg: Color::Rgb(20, 50, 20),
            diff_remove_bg: Color::Rgb(60, 20, 20),
            help_section_header_fg: Color::Cyan,
            input_cursor_fg: Color::Black,
            input_cursor_bg: Color::White,
            input_label_fg: Color::White,
            diff_binary_fg: Color::Yellow,
            diff_stat_add_fg: Color::Green,
            diff_stat_remove_fg: Color::Red,
            runtime_context_value_fg: Color::Rgb(125, 150, 160),
            nudge_border: Color::Rgb(180, 150, 50),
            tip_pill_fg: Color::Rgb(180, 180, 180),
            tip_pill_bg: Color::Rgb(70, 50, 120),
            tip_text_fg: Color::Rgb(90, 90, 90),
            tip_highlight_fg: Color::Rgb(0, 120, 120),
            diff_line_number_fg: Color::Rgb(90, 90, 110),
            diff_line_number_sep: Color::Rgb(60, 60, 70),
            pr_open_bg: Color::Rgb(35, 134, 54),
            pr_merged_bg: Color::Rgb(130, 80, 223),
            pr_closed_bg: Color::Rgb(110, 54, 48),
            pr_merged_label: Color::Rgb(170, 100, 220),
            pr_banner_fg: Color::White,
            pr_closed_label: Color::Rgb(140, 80, 80),
            pr_open_label: Color::Green,
            help_banner_fg: Color::Rgb(20, 20, 20),
            help_banner_bg: Color::Cyan,
            help_body_fg: Color::Rgb(180, 180, 180),
        }
    }

    /// Bundled dux-dark TOML must reproduce the original palette exactly,
    /// field for field. If a token in `assets/themes/dux-dark.toml` is
    /// retyped wrong, this test will pinpoint which one drifted.
    #[test]
    fn dux_dark_matches_original_palette() {
        let actual = load_from_str(DUX_DARK_TOML).expect("bundled dux-dark must parse");
        let expected = default_dark_archived();

        macro_rules! assert_field {
            ($field:ident) => {
                assert_eq!(
                    actual.$field,
                    expected.$field,
                    "field `{}` drifted: actual {:?} != expected {:?}",
                    stringify!($field),
                    actual.$field,
                    expected.$field
                );
            };
        }

        assert_field!(app_bg);
        assert_field!(text_fg);
        assert_field!(header_fg);
        assert_field!(header_bg);
        assert_field!(header_label_fg);
        assert_field!(header_separator_fg);
        assert_field!(border_focused);
        assert_field!(border_normal);
        assert_field!(title_focused);
        assert_field!(title_normal);
        assert_field!(selection_fg);
        assert_field!(selection_bg);
        assert_field!(project_icon);
        assert_field!(project_missing_fg);
        assert_field!(session_active);
        assert_field!(session_detached);
        assert_field!(session_exited);
        assert_field!(session_deleting);
        assert_field!(status_info_fg);
        assert_field!(status_info_bg);
        assert_field!(status_busy_fg);
        assert_field!(status_busy_bg);
        assert_field!(status_error_fg);
        assert_field!(status_error_bg);
        assert_field!(diff_add);
        assert_field!(diff_remove);
        assert_field!(diff_hunk);
        assert_field!(diff_file_header);
        assert_field!(file_status_fg);
        assert_field!(hint_key_fg);
        assert_field!(hint_bracket_fg);
        assert_field!(hint_key_bg);
        assert_field!(hint_desc_fg);
        assert_field!(hint_dim_key_fg);
        assert_field!(hint_dim_bracket_fg);
        assert_field!(hint_dim_desc_fg);
        assert_field!(hint_bar_bg);
        assert_field!(overlay_border);
        assert_field!(overlay_bg);
        assert_field!(overlay_dim_bg);
        assert_field!(prompt_cursor);
        assert_field!(provider_label_fg);
        assert_field!(branch_fg);
        assert_field!(terminal_hint_fg);
        assert_field!(scroll_indicator_fg);
        assert_field!(scroll_indicator_bg);
        assert_field!(warning_fg);
        assert_field!(button_active_fg);
        assert_field!(button_confirm_border);
        assert_field!(button_danger_border);
        assert_field!(overlay_dim_fg);
        assert_field!(diff_add_bg);
        assert_field!(diff_remove_bg);
        assert_field!(help_section_header_fg);
        assert_field!(input_cursor_fg);
        assert_field!(input_cursor_bg);
        assert_field!(input_label_fg);
        assert_field!(diff_binary_fg);
        assert_field!(diff_stat_add_fg);
        assert_field!(diff_stat_remove_fg);
        assert_field!(runtime_context_value_fg);
        assert_field!(nudge_border);
        assert_field!(tip_pill_fg);
        assert_field!(tip_pill_bg);
        assert_field!(tip_text_fg);
        assert_field!(tip_highlight_fg);
        assert_field!(diff_line_number_fg);
        assert_field!(diff_line_number_sep);
        assert_field!(pr_open_bg);
        assert_field!(pr_merged_bg);
        assert_field!(pr_closed_bg);
        assert_field!(pr_banner_fg);
        assert_field!(pr_merged_label);
        assert_field!(pr_closed_label);
        assert_field!(pr_open_label);
        assert_field!(help_banner_fg);
        assert_field!(help_banner_bg);
        assert_field!(help_body_fg);
    }

    /// The fallback path must always produce a valid Theme — it is what the
    /// app falls back to when the user's configured theme name is unknown.
    #[test]
    fn fallback_is_dux_dark() {
        let fallback = Theme::fallback();
        let archived = default_dark_archived();
        assert_eq!(fallback.border_focused, archived.border_focused);
        assert_eq!(fallback.header_bg, archived.header_bg);
        assert_eq!(fallback.diff_add, archived.diff_add);
        assert_eq!(fallback.app_bg, archived.app_bg);
    }

    /// An Opaline built-in (no `dux.*` tokens defined) should still produce a
    /// fully-populated Theme via the derivation registered in
    /// `register_dux_defaults`. This guards against the trap of `from_opaline`
    /// returning a sea of FALLBACK gray for non-dux themes.
    #[test]
    fn builtin_theme_loads_via_derivation() {
        let mut nord = opaline::load_by_name("nord").expect("nord built-in must exist");
        register_dux_defaults(&mut nord);
        let theme = Theme::from_opaline(&nord);
        // Nord's accent.primary is `nord8` (#88c0d0) — distinctly not the
        // FALLBACK gray, so this is a real assertion that derivation worked.
        assert_ne!(theme.border_focused, Color::Rgb(128, 128, 128));
        assert_ne!(theme.title_focused, Color::Rgb(128, 128, 128));
    }

    #[test]
    fn pr_colors_default_to_github_palette_not_theme_semantics() {
        let theme = load_from_str(
            r##"
[meta]
name = "Loud Semantic Theme"
variant = "dark"

[palette]
base = "#010203"
text = "#111111"
muted = "#222222"
dim = "#333333"
panel = "#444444"
highlight = "#555555"
active = "#666666"
accent = "#123456"
accent_secondary = "#abcdef"
border = "#999999"
success = "#010101"
error = "#020202"
warning = "#030303"
info = "#040404"

[tokens]
"text.primary" = "text"
"text.muted" = "muted"
"text.dim" = "dim"
"bg.base" = "base"
"bg.panel" = "panel"
"bg.highlight" = "highlight"
"bg.active" = "active"
"accent.primary" = "accent"
"accent.secondary" = "accent_secondary"
"border.focused" = "border"
"border.unfocused" = "dim"
success = "success"
error = "error"
warning = "warning"
info = "info"
"##,
        )
        .expect("theme must parse");

        assert_eq!(theme.pr_open_bg, Color::Rgb(35, 134, 54));
        assert_eq!(theme.pr_merged_bg, Color::Rgb(130, 80, 223));
        assert_eq!(theme.pr_closed_bg, Color::Rgb(110, 54, 48));
        assert_eq!(theme.pr_banner_fg, Color::White);
        assert_eq!(theme.pr_open_label, Color::Green);
        assert_eq!(theme.pr_merged_label, Color::Rgb(170, 100, 220));
        assert_eq!(theme.pr_closed_label, Color::Rgb(140, 80, 80));

        assert_ne!(theme.pr_open_bg, Color::Rgb(1, 1, 1));
        assert_ne!(theme.pr_merged_bg, Color::Rgb(171, 205, 239));
        assert_ne!(theme.pr_closed_bg, Color::Rgb(2, 2, 2));
    }

    #[test]
    fn explicit_pr_theme_tokens_override_github_defaults() {
        let theme = load_from_str(
            r##"
[meta]
name = "Custom PR Theme"
variant = "dark"

[tokens]
"dux.pr_open_bg" = "#010203"
"dux.pr_merged_bg" = "#040506"
"dux.pr_closed_bg" = "#070809"
"dux.pr_banner_fg" = "#101112"
"dux.pr_open_label" = "#131415"
"dux.pr_merged_label" = "#161718"
"dux.pr_closed_label" = "#192021"
"##,
        )
        .expect("theme must parse");

        assert_eq!(theme.pr_open_bg, Color::Rgb(1, 2, 3));
        assert_eq!(theme.pr_merged_bg, Color::Rgb(4, 5, 6));
        assert_eq!(theme.pr_closed_bg, Color::Rgb(7, 8, 9));
        assert_eq!(theme.pr_banner_fg, Color::Rgb(16, 17, 18));
        assert_eq!(theme.pr_open_label, Color::Rgb(19, 20, 21));
        assert_eq!(theme.pr_merged_label, Color::Rgb(22, 23, 24));
        assert_eq!(theme.pr_closed_label, Color::Rgb(25, 32, 33));
    }
}
