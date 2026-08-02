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
//! Each question is split into a function TAKING the resolved home and a thin
//! wrapper that resolves it, so "the home directory cannot be resolved" is
//! testable without mutating the process environment (which is unsafe in a
//! threaded test runner, and which the repo's git-isolation work already got
//! bitten by). The WHERE half also probes the filesystem, because a path is not
//! yet a directory a shell can start in (see [`dir_is_usable`]); the WRITTEN
//! half stays pure string work.

use std::path::{Path, PathBuf};

/// Whether `path` is a directory dux could actually start a shell in: it
/// resolves, it is a directory, and dux may search it.
///
/// One `stat` on `path/.` answers all three, using the process's EFFECTIVE
/// credentials (unlike `access(2)`, which asks about the real ones). Resolving
/// the trailing `.` component requires SEARCH permission on the directory, so a
/// `0o000` directory reports `EACCES`; a regular file reports `ENOTDIR`; and
/// because the walk follows symlinks, a dangling link reports `ENOENT`. The
/// `is_dir` check is then belt and braces rather than the load-bearing part.
///
/// This is a probe, so it is inherently a moment in time: a directory can be
/// removed between the check and the spawn. It is not a guarantee, it is the
/// difference between the common unusable-home cases costing the user a
/// terminal and costing them nothing.
pub fn dir_is_usable(path: &Path) -> bool {
    std::fs::metadata(path.join("."))
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

/// The directory a standalone terminal opens in, given the resolved home.
///
/// `None` means the home directory could not be resolved at all (no `$HOME`, no
/// passwd entry). That is not a reason to refuse to open a terminal: the user
/// asked for a shell and a shell can run anywhere, so it falls back to the
/// filesystem root, which exists on every platform dux supports.
///
/// A home that resolves but is NOT USABLE ([`dir_is_usable`]) takes the same
/// fallback, because it is the same situation from the user's side. The
/// directory is handed to the spawn as the child's working directory, so a home
/// that is a regular file, a dangling symlink, or a directory dux may not
/// search fails the spawn outright and the terminal is never created. That is a
/// clean failure, and a useless one: the fallback exists so that a shell still
/// runs somewhere.
///
/// Note what this deliberately does NOT do: it chooses a DIRECTORY, it does not
/// retry a failed spawn. A missing shell or a bad `[terminal] command` is a
/// configuration problem the user needs to see, and retrying it at `/` would
/// bury it.
pub fn standalone_terminal_dir_from(home: Option<PathBuf>) -> PathBuf {
    match home {
        Some(home) if dir_is_usable(&home) => home,
        _ => PathBuf::from("/"),
    }
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
        // A REAL directory, because "the home directory" now means one dux can
        // actually spawn in (see the unusable cases below).
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().to_path_buf();
        assert!(dir_is_usable(&home));
        assert_eq!(standalone_terminal_dir_from(Some(home.clone())), home);
    }

    #[test]
    fn the_real_home_is_where_a_standalone_terminal_opens() {
        // The ordinary case, end to end through the resolver: a real machine's
        // home is an ordinary directory, so that is where the terminal opens.
        // Where no home resolves, or it is somehow unusable, the fallback tests
        // below own the answer and there is nothing to assert here.
        let Some(home) = home::home_dir().filter(|h| dir_is_usable(h)) else {
            return;
        };
        assert_eq!(standalone_terminal_dir(), home);
    }

    #[test]
    fn an_unresolvable_home_opens_at_the_root_rather_than_failing() {
        // The user asked for a shell. A shell can run anywhere, so dux opens one
        // at `/` instead of refusing.
        assert_eq!(standalone_terminal_dir_from(None), PathBuf::from("/"));
    }

    /// A home that RESOLVES is not necessarily a home dux can spawn in, and the
    /// directory goes straight to the child as its working directory: an
    /// unusable one fails the spawn, so no terminal is created at all. That
    /// defeats the point of the fallback, so each unusable shape falls back to
    /// `/` exactly as an unresolvable home does.
    #[test]
    fn a_home_that_is_not_a_directory_falls_back_to_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("home");
        std::fs::write(&file, b"not a directory").expect("write file");
        assert!(!dir_is_usable(&file));
        assert_eq!(
            standalone_terminal_dir_from(Some(file)),
            PathBuf::from("/"),
            "a regular file where home should be is not somewhere a shell can run"
        );
    }

    #[test]
    fn a_dangling_symlink_home_falls_back_to_the_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let link = tmp.path().join("home");
        std::os::unix::fs::symlink(tmp.path().join("gone"), &link).expect("symlink");
        // The link itself is right there; what it points at is not, which is why
        // the check has to RESOLVE symlinks rather than stat the link.
        assert!(link.symlink_metadata().is_ok());
        assert!(!dir_is_usable(&link));
        assert_eq!(standalone_terminal_dir_from(Some(link)), PathBuf::from("/"));
    }

    #[test]
    fn a_home_without_search_permission_falls_back_to_the_root() {
        // Root bypasses the permission bits entirely (CAP_DAC_OVERRIDE), so this
        // case cannot be built there; skipping beats asserting the opposite of
        // what an ordinary user sees.
        if rustix::process::geteuid().is_root() {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("home");
        std::fs::create_dir(&dir).expect("create dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let usable = dir_is_usable(&dir);
        let fallback = standalone_terminal_dir_from(Some(dir.clone()));
        // Restored before asserting, so a failing run still leaves a removable
        // tempdir rather than an undeletable one.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod back");
        assert!(
            !usable,
            "a directory dux cannot search is not one it can spawn in"
        );
        assert_eq!(fallback, PathBuf::from("/"));
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
