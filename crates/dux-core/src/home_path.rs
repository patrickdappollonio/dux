//! The two questions dux asks about the user's home directory when it opens a
//! standalone terminal: WHERE that terminal starts, and how the directory is
//! WRITTEN in the sidebar row that names it.
//!
//! Both are deliberately answered from `home::home_dir()` and nothing else.
//! `Paths::config_root` (see [`crate::config`]) also consults `DUX_HOME` and
//! `XDG_CONFIG_HOME`, because it is answering a different question: where dux
//! keeps ITS OWN files. A terminal opening under `$XDG_CONFIG_HOME` because the
//! user redirected dux's config would surprise everybody.
//!
//! Each question is split into a pure function taking the resolved home and a
//! thin wrapper that resolves it, so "the home directory cannot be resolved" is
//! testable without mutating the process environment (which is unsafe in a
//! threaded test runner, and which the repo's git-isolation work already got
//! bitten by).

use std::path::{Path, PathBuf};

/// The directory a standalone terminal opens in, given the resolved home.
///
/// `None` means the home directory could not be resolved at all (no `$HOME`, no
/// passwd entry). That is not a reason to refuse to open a terminal: the user
/// asked for a shell and a shell can run anywhere, so it falls back to the
/// filesystem root, which exists on every platform dux supports.
pub fn standalone_terminal_dir_from(home: Option<PathBuf>) -> PathBuf {
    home.unwrap_or_else(|| PathBuf::from("/"))
}

/// The directory a standalone terminal opens in. See
/// [`standalone_terminal_dir_from`] for the rule and why the home directory is
/// resolved this way rather than through the config-directory resolver.
pub fn standalone_terminal_dir() -> PathBuf {
    standalone_terminal_dir_from(home::home_dir())
}

/// `path` written for a human, with the home directory collapsed to `~`, given
/// the resolved home. The home directory itself is exactly `~`; a path beneath
/// it is `~/code`; anything else (including every path when home cannot be
/// resolved) is returned verbatim.
///
/// This is the standalone terminal's second sidebar line, so it is also what the
/// sidebar search matches. It ellipsizes from the right like every other left
/// element on that line, which is why the SHORT end (`~/…`) is the end kept.
///
/// Not to be confused with `engine::portable_project_path`, which writes the
/// same collapse as `$HOME/...` for config.toml. That one is machine-portable
/// desired state that gets expanded again on load; this one is display text
/// nothing ever parses back.
pub fn shorten_home_from(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.to_string_lossy().into_owned();
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    }
}

/// [`shorten_home_from`] against the process's resolved home directory.
pub fn shorten_home(path: &Path) -> String {
    shorten_home_from(path, home::home_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_standalone_terminal_opens_in_the_home_directory() {
        assert_eq!(
            standalone_terminal_dir_from(Some(PathBuf::from("/home/ada"))),
            PathBuf::from("/home/ada")
        );
    }

    #[test]
    fn an_unresolvable_home_opens_at_the_root_rather_than_failing() {
        // The user asked for a shell. A shell can run anywhere, so dux opens one
        // at `/` instead of refusing.
        assert_eq!(standalone_terminal_dir_from(None), PathBuf::from("/"));
    }

    #[test]
    fn the_home_directory_itself_is_written_as_a_bare_tilde() {
        assert_eq!(
            shorten_home_from(Path::new("/home/ada"), Some(Path::new("/home/ada"))),
            "~"
        );
    }

    #[test]
    fn a_directory_under_home_is_written_with_a_tilde_prefix() {
        assert_eq!(
            shorten_home_from(Path::new("/home/ada/code"), Some(Path::new("/home/ada"))),
            "~/code"
        );
        assert_eq!(
            shorten_home_from(
                Path::new("/home/ada/code/dux"),
                Some(Path::new("/home/ada"))
            ),
            "~/code/dux"
        );
    }

    #[test]
    fn a_directory_outside_home_is_written_verbatim() {
        assert_eq!(
            shorten_home_from(Path::new("/srv/build"), Some(Path::new("/home/ada"))),
            "/srv/build"
        );
        // A sibling whose name merely STARTS with the home directory's is not
        // under it: `strip_prefix` compares components, not characters.
        assert_eq!(
            shorten_home_from(Path::new("/home/adamant"), Some(Path::new("/home/ada"))),
            "/home/adamant"
        );
    }

    #[test]
    fn an_unresolvable_home_leaves_the_path_alone() {
        assert_eq!(shorten_home_from(Path::new("/"), None), "/");
        assert_eq!(shorten_home_from(Path::new("/home/ada"), None), "/home/ada");
    }
}
