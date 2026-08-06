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
//! Two rules hold everywhere in here. Tightening is **best effort**: a mode dux
//! cannot set is a warning, never a reason to stop logging, stop opening the
//! database, or refuse to start. And dux **never chmods through a symlink**,
//! because the mode it would change belongs to somebody else's file.
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
///
/// A SYMLINK is skipped, with a warning, and that is deliberate. Both
/// `fs::metadata` and `fs::set_permissions` follow links, so tightening a
/// symlinked path changed the mode of whatever it pointed at: measured, a
/// config directory symlinked to a shared directory left the TARGET at `0700`,
/// and a `config.toml` symlinked into a dotfiles repository had dux changing
/// the mode of a file inside that repository. That is a common setup. The
/// reasoning is the same one [`create_private_dir_all`] already gives for
/// parents: quietly making somebody else's file owner-only is a surprising
/// thing for dux to do, and the surprise is just as available at the root
/// itself as it is one level up. Skipping is the right answer rather than
/// following-and-tightening or refusing to start, because the user who made
/// the link is the one who chose where the file really lives, and dux's actual
/// enforcement is the config directory's own mode either way.
///
/// The check and the chmod are two syscalls, so a path swapped for a symlink
/// between them would still be followed. That is not defended against: dux is
/// single-tenant and this is the user's own config directory, so the case
/// requires an attacker who already has the access the mode is protecting.
pub fn restrict_to_owner(path: &Path) -> io::Result<()> {
    // `symlink_metadata` does NOT follow, which is the whole point; for a
    // non-symlink it answers exactly what `metadata` would.
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if meta.file_type().is_symlink() {
        crate::logger::warn(&format!(
            "{} is a symlink, so its permissions were left alone; \
             dux does not change the mode of a link's target",
            path.display()
        ));
        return Ok(());
    }
    let current = meta.permissions().mode();
    let tightened = current & !GROUP_AND_OTHER;
    if tightened == current {
        return Ok(());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(tightened))
}

/// Tighten `path` if it can be tightened, and warn rather than fail if it
/// cannot.
///
/// Permissions are a hardening measure, never a precondition for dux working.
/// A path dux can write to but cannot `chmod` is reachable through ordinary
/// configuration (a `logging.path` under `/var/log` owned by an admin, a
/// Windows-mounted path under WSL2, a FAT or NFS volume), and on every one of
/// those the alternative is worse than a loose mode: propagating the error
/// turned "dux logs to a slightly loose file" into "dux does not log at all and
/// says nothing", and turned an existing, working config directory into a
/// refusal to start. `what` names the thing for the warning.
pub fn restrict_to_owner_best_effort(path: &Path, what: &str) {
    if let Err(err) = restrict_to_owner(path) {
        crate::logger::warn(&format!(
            "could not restrict permissions on the {what} at {}: {err}. \
             dux will carry on; tighten it by hand if the mode matters to you",
            path.display()
        ));
    }
}

/// Create `path` and any missing parents, then tighten `path` itself to
/// owner-only. The create honours the process umask, which on a typical `0022`
/// leaves `0755`, so the tightening is not optional.
///
/// Only the final component is tightened. Parents may be directories dux does
/// not own (a `~/.config` shared with every other tool), and quietly making one
/// of those owner-only would be a surprising thing for dux to do to somebody
/// else's directory.
///
/// Creation is fatal; the tightening is not. A directory that already exists
/// and works must not become a startup failure because its mode could not be
/// changed, and reporting that as `failed to create <path>` named the wrong
/// thing entirely: the directory was there the whole time.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    restrict_to_owner_best_effort(path, "directory");
    Ok(())
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

    /// `fs::metadata` and `fs::set_permissions` both FOLLOW symlinks, so
    /// tightening a symlinked path changed the mode of whatever it pointed at.
    /// A `config.toml` symlinked into a dotfiles repository is a common setup,
    /// and dux silently making a file inside that repository owner-only is
    /// exactly the surprise this module already declines to inflict on parent
    /// directories.
    #[test]
    fn restrict_does_not_chmod_through_a_symlink_to_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles-config.toml");
        fs::write(&target, "x").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        restrict_to_owner(&link).unwrap();

        assert_eq!(
            mode_of(&target),
            0o644,
            "the symlink TARGET must not have been tightened"
        );
    }

    #[test]
    fn restrict_does_not_chmod_through_a_symlink_to_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("shared");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.path().join("dux");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        restrict_to_owner(&link).unwrap();

        assert_eq!(
            mode_of(&target),
            0o755,
            "the symlink TARGET must not have been tightened"
        );
    }

    /// The config-root case: `create_dir_all` succeeds because the directory is
    /// already there through the link, and the tightening is then skipped. dux
    /// must still start.
    ///
    /// This is NOT the best-effort test, though it reads like one: the symlink
    /// branch returns `Ok(())` before any chmod is attempted, so a fatal
    /// version of the tightening would pass here too. The chmod-actually-fails
    /// case is pinned by
    /// `create_private_dir_all_still_succeeds_when_the_leaf_cannot_be_tightened`.
    #[test]
    fn create_private_dir_all_through_a_symlink_succeeds_and_leaves_the_target_alone() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("shared");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
        let link = dir.path().join("dux");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        create_private_dir_all(&link).expect("a directory dux cannot tighten must not be fatal");

        assert_eq!(mode_of(&target), 0o755);
    }

    /// Permissions are best-effort, but CREATION is not: a directory that
    /// genuinely could not be made must still fail, and must fail about that.
    #[test]
    fn create_private_dir_all_still_fails_when_the_directory_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("in-the-way");
        fs::write(&blocker, "not a directory").unwrap();
        assert!(
            create_private_dir_all(&blocker.join("child")).is_err(),
            "a real creation failure must still be reported"
        );
    }

    /// The strict form must REPORT a chmod it could not do, or the best-effort
    /// wrapper below has nothing to swallow and its whole existence is
    /// untested. This is the shape that builds it unprivileged, MEASURED:
    /// `symlink_metadata` on a path under a REGULAR FILE answers
    /// `NotADirectory` (os error 20), which is not `NotFound` and so must
    /// surface.
    #[test]
    fn restrict_to_owner_reports_a_failure_that_is_not_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular");
        fs::write(&regular, "x").unwrap();

        let err = restrict_to_owner(&regular.join("child"))
            .expect_err("a path that is not merely missing must surface");
        assert_eq!(
            err.kind(),
            io::ErrorKind::NotADirectory,
            "expected the real errno, got {err:?}"
        );
    }

    /// And the best-effort wrapper must swallow exactly that error. Nothing
    /// else in this module pins it: the two symlink tests look like coverage
    /// but return `Ok(())` before any chmod is attempted, so there is no error
    /// on those paths for a fatal version to propagate.
    #[test]
    fn restrict_to_owner_best_effort_swallows_an_error_the_strict_form_returns() {
        let dir = tempfile::tempdir().unwrap();
        let regular = dir.path().join("regular");
        fs::write(&regular, "x").unwrap();
        let unreachable = regular.join("child");
        assert!(
            restrict_to_owner(&unreachable).is_err(),
            "the test is only meaningful while the strict form fails here"
        );

        // No return value to assert on: not panicking and not propagating IS
        // the behaviour. A fatal version could not compile at this call.
        restrict_to_owner_best_effort(&unreachable, "test path");
    }

    /// The headline guarantee at the call site: a directory that EXISTS and
    /// works must not become a startup failure because dux cannot change its
    /// mode. Making the tightening fatal here breaks this test and nothing
    /// else in the crate, which is why it is written.
    ///
    /// MEASURED shape: `/proc/self/fdinfo` is a real directory at mode `0555`,
    /// so `create_dir_all` succeeds and the tightening is attempted, and procfs
    /// refuses a mode change with `EPERM` for EVERY caller including root
    /// (`proc_setattr` rejects `ATTR_MODE` outright), so the test neither needs
    /// a privilege gate nor risks changing anything. Linux-only, hence the
    /// stated skip rather than a silent pass.
    #[test]
    fn create_private_dir_all_still_succeeds_when_the_leaf_cannot_be_tightened() {
        let path = Path::new("/proc/self/fdinfo");
        if !path.is_dir() {
            eprintln!("SKIPPED: no procfs here, so an un-chmod-able directory cannot be built");
            return;
        }
        let before = mode_of(path);
        assert_ne!(
            before & GROUP_AND_OTHER,
            0,
            "the test is only meaningful while a tightening is actually attempted"
        );
        assert!(
            restrict_to_owner(path).is_err(),
            "the test is only meaningful while the chmod really fails"
        );

        create_private_dir_all(path)
            .expect("a directory dux cannot tighten must not be a startup failure");

        assert_eq!(mode_of(path), before, "nothing should have been changed");
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
