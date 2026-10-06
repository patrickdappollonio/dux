//! Surface-agnostic theme identity: the default theme's name, and which
//! themes dux can load.
//!
//! The full theme model (the ratatui `Theme` struct, opaline-backed color
//! loading, and `Style`/`Span` helpers) is rendering-specific and lives in the
//! TUI surface (its loaders also depend on config/logger). The listing is here
//! because the terminal UI's picker, `dux themes ls` and the web API's theme
//! route must name the same themes in the same order.

use crate::config::DuxPaths;

/// Name of the bundled default theme, also the value written into the
/// generated `config.toml` on first boot.
pub const DEFAULT_THEME_NAME: &str = "dux_dark";

/// Where a theme came from, used by the theme picker to label entries and
/// disambiguate same-named user themes from built-ins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// The bundled `dux-dark` theme: always present, always first in the list.
    Bundled,
    /// A theme compiled into the opaline crate (e.g. `nord`, `catppuccin-mocha`).
    Opaline,
    /// A user-authored theme found at `<config_dir>/themes/<name>.toml`.
    User,
}

impl ThemeSource {
    /// The word the picker and `dux themes ls` label a theme's source with.
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeSource::Bundled => "bundled",
            ThemeSource::Opaline => "opaline",
            ThemeSource::User => "user",
        }
    }
}

/// Metadata about an available theme, used to populate the theme picker.
#[derive(Clone, Debug)]
pub struct ThemeListing {
    /// Identifier the terminal UI loads it by: file stem for user themes,
    /// underscored id for built-ins, `dux_dark` for the bundled default.
    pub id: String,
    /// Human-readable label shown in the picker.
    pub display_name: String,
    pub source: ThemeSource,
}

/// Enumerate every theme reachable from the dux runtime: the bundled
/// `dux-dark`, the opaline built-ins, and any TOML files the user has
/// dropped in `<config_dir>/themes/`. Sorted with `dux-dark` first, then
/// user themes, then built-ins alphabetically, for predictable scrolling.
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
                    // same name still loads first via the terminal UI's
                    // loader, but we don't show two "dux-dark" rows in the
                    // picker.
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
            // user-facing id; the terminal UI's loader reverses the
            // conversion before calling opaline.
            id: info.name.replace('-', "_"),
            display_name: info.display_name.clone(),
            source: ThemeSource::Opaline,
        })
        .collect();
    builtin.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    themes.extend(builtin);

    themes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes_list_the_bundled_one_then_the_users_by_name_then_the_built_ins_by_display_name() {
        let tmp = crate::test_scratch::ScratchDir::new();
        let themes = tmp.path().join("themes");
        std::fs::create_dir_all(&themes).unwrap();
        for file in ["zeta.toml", "alpha.toml", "dux_dark.toml", "notes.txt"] {
            std::fs::write(themes.join(file), "").unwrap();
        }
        let root = tmp.path();
        let paths = DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };
        let listed: Vec<(String, &str)> = discover_available(&paths)
            .into_iter()
            .map(|theme| (theme.id, theme.source.as_str()))
            .collect();
        assert_eq!(
            listed[..5],
            [
                ("dux_dark".to_string(), "bundled"),
                ("alpha".to_string(), "user"),
                ("zeta".to_string(), "user"),
                ("ayu_dark".to_string(), "opaline"),
                ("ayu_light".to_string(), "opaline"),
            ]
        );
        let position = |id: &str| listed.iter().position(|(name, _)| name == id);
        assert!(position("ayu_mirage") < position("catppuccin_frappe"));
        assert!(position("catppuccin_mocha") < position("nord"));
    }
}
