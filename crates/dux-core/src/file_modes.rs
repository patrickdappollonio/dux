//! The one place that decides what permissions dux's OWN files and directories
//! get. Every file dux keeps for itself lives in the config directory, and the
//! rule is the same for all of them: the owner, and nobody else.
//!
//! `config.toml` has always been `0600`, because it may hold tokens under
//! `[env]`. The rest of the directory did not follow: `sessions.sqlite3` mirrors
//! the same per-project `env` map, and it, its `-wal`/`-shm` sidecars, and
//! `dux.log` were all created at the umask default, typically `0644`.
//!
//! **The directory mode is what actually closes this**, and it is why the rule
//! lives here rather than as three separate decisions at three call sites.
//! SQLite creates the sidecar files ITSELF, at runtime, after the connection is
//! open, and offers no API to set their mode, so there is no moment at which
//! dux can get in front of them. A `0700` directory makes that moot: another
//! local user who cannot traverse the directory cannot reach a file inside it
//! whatever mode the file carries. The per-file modes below are defence in
//! depth for the case where a file is copied out of the directory, or the
//! directory's mode is later loosened by hand.
//!
//! Unix-only, deliberately. dux targets macOS and Linux (see CLAUDE.md), so
//! there is no `cfg(windows)` branch here.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A file dux writes for itself: owner read/write, `0600`.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// A directory dux owns: owner read/write/search, `0700`. Search permission is
/// the load-bearing bit, since it is what another user needs to reach anything
/// inside.
pub const PRIVATE_DIR_MODE: u32 = 0o700;

/// The group and other bits, the ones dux always clears from its own files.
const GROUP_AND_OTHER: u32 = 0o077;

/// Strip every group and other permission bit from an existing path, leaving the
/// owner bits exactly as they are.
///
/// This TIGHTENS and never loosens, which is what makes it safe to run on every
/// startup over an installation created before dux cared: a `0755` directory
/// becomes `0700` and a `0644` file becomes `0600`, while a user who has
/// deliberately made their config read-only at `0400` keeps it. It is
/// idempotent, so the second startup changes nothing.
///
/// A missing path is NOT an error: the caller often lists every file dux might
/// keep (the sqlite sidecars in particular exist only sometimes), and a path
/// that is not there needs no tightening. Any other error is returned.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let current = meta.permissions().mode();
    let tightened = current & !GROUP_AND_OTHER;
    if tightened == current {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(tightened))
}

/// Create `path` and any missing parents, then tighten `path` itself to
/// owner-only. The create honours the process umask, which on a typical `0022`
/// leaves `0755`, so the tightening is not optional.
///
/// Only the final component is tightened. Parents may be directories dux does
/// not own (a `~/.config` shared with every other tool), and quietly making one
/// of those owner-only would be a surprising thing for dux to do to somebody
/// else's directory.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    restrict_to_owner(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn restrict_strips_group_and_other_from_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        restrict_to_owner(&file).unwrap();
        assert_eq!(mode_of(&file), PRIVATE_FILE_MODE);
    }

    #[test]
    fn restrict_strips_group_and_other_from_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("d");
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        restrict_to_owner(&sub).unwrap();
        assert_eq!(mode_of(&sub), PRIVATE_DIR_MODE);
    }

    #[test]
    fn restrict_preserves_owner_bits_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        // A deliberately read-only config must stay read-only, not be widened
        // back to 0600.
        fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
        restrict_to_owner(&file).unwrap();
        assert_eq!(mode_of(&file), 0o400);
        restrict_to_owner(&file).unwrap();
        assert_eq!(mode_of(&file), 0o400);
    }

    #[test]
    fn restrict_never_loosens() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        restrict_to_owner(&file).unwrap();
        assert_eq!(mode_of(&file), 0o600);
    }

    #[test]
    fn restrict_treats_a_missing_path_as_nothing_to_do() {
        let dir = tempfile::tempdir().unwrap();
        restrict_to_owner(&dir.path().join("never-existed")).unwrap();
    }

    #[test]
    fn create_private_dir_all_makes_the_leaf_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a").join("b");
        create_private_dir_all(&leaf).unwrap();
        assert_eq!(mode_of(&leaf), PRIVATE_DIR_MODE);
    }

    #[test]
    fn create_private_dir_all_leaves_parents_alone() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("shared");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        create_private_dir_all(&parent.join("mine")).unwrap();
        assert_eq!(
            mode_of(&parent),
            0o755,
            "a parent dux does not own must not be tightened"
        );
    }
}
