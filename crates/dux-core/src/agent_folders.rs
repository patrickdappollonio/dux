//! The folders dux creates for standalone agents, under the agents folder
//! (`[defaults] standalone_agents_directory`, `<dux folder>/agents` when unset).
//!
//! Creating one refuses a name that is already taken, atomically, and records
//! the new directory's identity. Removing one deletes it only while the
//! directory at its path is still the one dux created, reached without
//! following a symbolic link anywhere inside it. Every function here touches
//! the filesystem and belongs on a worker or a blocking pool.

use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use crate::model::DirectoryIdentity;

/// The name of the folder a new standalone agent called `agent_name` gets:
/// the name with every `/` turned into `-`, so the folder sits directly in the
/// agents folder. The web's `agentFolderName` is its twin.
pub fn folder_name_for(agent_name: &str) -> String {
    agent_name.trim().replace('/', "-")
}

/// Where a new standalone agent called `agent_name` would run.
pub fn folder_path_for(root: &Path, agent_name: &str) -> PathBuf {
    root.join(folder_name_for(agent_name))
}

/// A folder dux has just created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedAgentFolder {
    /// The folder, with the agents folder's own path resolved.
    pub path: PathBuf,
    /// The identity the new directory has.
    pub identity: DirectoryIdentity,
}

/// Why a folder was not created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateAgentFolderError {
    /// Something is already at the folder's path, so nothing was created.
    AlreadyThere(PathBuf),
    /// The folder could not be created; the text says why.
    Failed {
        /// The folder that was to be created.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
}

/// Create the folder for a new standalone agent called `agent_name` inside
/// `root`, creating `root` first when it is missing.
///
/// The folder itself is created with one `mkdirat` relative to the opened
/// agents folder, so a folder that is already there, or one created by anyone
/// else at the same moment, is refused rather than shared.
///
/// # Errors
///
/// [`CreateAgentFolderError::AlreadyThere`] when the name is taken, and
/// [`CreateAgentFolderError::Failed`] when the agents folder or the new folder
/// cannot be created or read.
pub fn create_agent_folder(
    root: &Path,
    agent_name: &str,
) -> Result<CreatedAgentFolder, CreateAgentFolderError> {
    let name = folder_name_for(agent_name);
    let failed = |path: &Path, reason: String| CreateAgentFolderError::Failed {
        path: path.to_path_buf(),
        reason,
    };
    if name.is_empty() || name == "." || name == ".." {
        return Err(failed(
            &root.join(&name),
            "an agent needs a name to name its folder after".to_string(),
        ));
    }
    std::fs::create_dir_all(root).map_err(|err| failed(&root.join(&name), err.to_string()))?;
    let root =
        std::fs::canonicalize(root).map_err(|err| failed(&root.join(&name), err.to_string()))?;
    let path = root.join(&name);
    let root_fd = rustix::fs::open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| failed(&path, err.to_string()))?;
    match rustix::fs::mkdirat(&root_fd, name.as_str(), Mode::from_bits_truncate(0o777)) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => return Err(CreateAgentFolderError::AlreadyThere(path)),
        Err(err) => return Err(failed(&path, err.to_string())),
    }
    let stat = rustix::fs::statat(&root_fd, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|err| failed(&path, err.to_string()))?;
    Ok(CreatedAgentFolder {
        path,
        identity: identity_of(&stat),
    })
}

/// Remove an empty folder dux created moments ago for an agent that never
/// came up. A folder anything was written into is kept.
pub fn remove_if_empty(path: &Path) {
    if let Err(err) = std::fs::remove_dir(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        crate::logger::warn(&format!(
            "kept the folder {} dux created for an agent that did not start: {err}",
            path.display()
        ));
    }
}

/// What removing a folder dux created did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovedFolder {
    /// The folder and everything in it were deleted.
    Removed,
    /// Nothing was at the folder's path any more.
    AlreadyGone,
}

/// Delete the folder dux created at `path`, and everything in it, while the
/// directory there still has `identity`.
///
/// Nothing is followed: a folder on the way to `path` that is now a symbolic
/// link, a symbolic link or a file at `path`, or a directory there with
/// another identity is refused and left alone. Inside the folder a symbolic
/// link is removed as a link and never followed, and every entry is reached
/// relative to the directory that was checked.
///
/// # Errors
///
/// The sentence fragment saying why the folder was kept, or what stopped the
/// deletion part way through.
pub fn remove_created_folder(
    path: &Path,
    identity: DirectoryIdentity,
) -> Result<RemovedFolder, String> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err("its path does not name a folder inside another one".to_string());
    };
    match std::fs::canonicalize(parent) {
        Ok(resolved) if resolved == parent => {}
        Ok(_) => {
            return Err(
                "a folder on the way to it is now a symbolic link, and dux does not follow one \
                 when it deletes"
                    .to_string(),
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemovedFolder::AlreadyGone);
        }
        Err(err) => return Err(format!("dux could not read the folder it is in ({err})")),
    }
    let parent_fd = rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| format!("dux could not open the folder it is in ({err})"))?;
    let stat = match rustix::fs::statat(&parent_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(RemovedFolder::AlreadyGone),
        Err(err) => return Err(format!("dux could not read it ({err})")),
    };
    check_is_the_created_folder(&stat, identity)?;
    let dir_fd = rustix::fs::openat(
        &parent_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| format!("dux could not open it ({err})"))?;
    let opened =
        rustix::fs::fstat(&dir_fd).map_err(|err| format!("dux could not read it ({err})"))?;
    check_is_the_created_folder(&opened, identity)?;
    remove_contents(&dir_fd)
        .map_err(|err| format!("deleting what it holds stopped part way ({err})"))?;
    match rustix::fs::unlinkat(&parent_fd, name, AtFlags::REMOVEDIR) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(RemovedFolder::Removed),
        Err(err) => Err(format!(
            "its contents were deleted but the folder itself was not ({err}); something may \
             have written into it meanwhile"
        )),
    }
}

/// Refuse anything at the folder's path other than the directory dux created.
fn check_is_the_created_folder(
    stat: &rustix::fs::Stat,
    identity: DirectoryIdentity,
) -> Result<(), String> {
    match FileType::from_raw_mode(stat.st_mode) {
        FileType::Directory => {}
        FileType::Symlink => {
            return Err(
                "it is now a symbolic link, and dux does not follow one when it deletes"
                    .to_string(),
            );
        }
        _ => return Err("it is no longer a folder".to_string()),
    }
    if identity_of(stat) != identity {
        return Err(
            "the folder at that path is no longer the one dux created: something else was put \
             there since"
                .to_string(),
        );
    }
    Ok(())
}

/// Delete everything inside the opened directory, never following a link.
fn remove_contents(dir: &rustix::fd::OwnedFd) -> rustix::io::Result<()> {
    let mut entries = rustix::fs::Dir::read_from(dir)?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        names.push(name.to_owned());
    }
    for name in names {
        let stat = match rustix::fs::statat(dir, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(err) => return Err(err),
        };
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let child = rustix::fs::openat(
                dir,
                name.as_c_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            remove_contents(&child)?;
            drop(child);
            ignore_missing(rustix::fs::unlinkat(
                dir,
                name.as_c_str(),
                AtFlags::REMOVEDIR,
            ))?;
        } else {
            ignore_missing(rustix::fs::unlinkat(dir, name.as_c_str(), AtFlags::empty()))?;
        }
    }
    Ok(())
}

fn ignore_missing(result: rustix::io::Result<()>) -> rustix::io::Result<()> {
    match result {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        other => other,
    }
}

fn identity_of(stat: &rustix::fs::Stat) -> DirectoryIdentity {
    // `st_dev` and `st_ino` are unsigned on every Unix dux runs on; the
    // conversion only widens.
    #[expect(
        clippy::useless_conversion,
        reason = "st_dev is u32 on some targets and u64 on others"
    )]
    DirectoryIdentity {
        device: u64::from(stat.st_dev),
        inode: u64::from(stat.st_ino),
    }
}

/// How many files a folder holds, for the delete dialog's warning.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FolderFiles {
    /// The folder is there and holds `count` files (every entry that is not a
    /// folder, links included, at any depth).
    Counted {
        /// The number of files.
        count: u64,
    },
    /// Nothing is at the folder's path.
    Missing,
    /// The folder, or something inside it, could not be read.
    Unreadable,
}

/// Count the files in `path`, never following a symbolic link.
pub fn count_folder_files(path: &Path) -> FolderFiles {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return FolderFiles::Unreadable,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return FolderFiles::Missing,
        Err(_) => return FolderFiles::Unreadable,
    }
    let mut count = 0_u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false).min_depth(1) {
        match entry {
            Ok(entry) if entry.file_type().is_dir() => {}
            Ok(_) => count += 1,
            Err(_) => return FolderFiles::Unreadable,
        }
    }
    FolderFiles::Counted { count }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn a_new_folder_is_created_inside_a_missing_agents_folder() {
        let tmp = root();
        let agents = tmp.path().join("dux").join("agents");
        let created = create_agent_folder(&agents, "brave-otter").expect("created");
        assert_eq!(
            created.path,
            std::fs::canonicalize(&agents).unwrap().join("brave-otter")
        );
        assert!(created.path.is_dir());
        assert_eq!(
            created.identity,
            DirectoryIdentity::of(&created.path).unwrap()
        );
    }

    #[test]
    fn a_name_that_is_already_there_is_refused_and_nothing_is_touched() {
        let tmp = root();
        std::fs::create_dir(tmp.path().join("taken")).unwrap();
        std::fs::write(tmp.path().join("taken/keep.txt"), "mine").unwrap();
        std::fs::write(tmp.path().join("a-file"), "mine").unwrap();
        assert!(matches!(
            create_agent_folder(tmp.path(), "taken"),
            Err(CreateAgentFolderError::AlreadyThere(_))
        ));
        assert!(matches!(
            create_agent_folder(tmp.path(), "a-file"),
            Err(CreateAgentFolderError::AlreadyThere(_))
        ));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("taken/keep.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn a_dangling_symbolic_link_at_the_name_is_refused_and_not_followed() {
        let tmp = root();
        let target = tmp.path().join("elsewhere");
        std::os::unix::fs::symlink(&target, tmp.path().join("linked")).unwrap();
        assert!(matches!(
            create_agent_folder(tmp.path(), "linked"),
            Err(CreateAgentFolderError::AlreadyThere(_))
        ));
        assert!(!target.exists(), "the link's target must not be created");
    }

    #[test]
    fn two_creates_of_one_name_at_once_create_it_once() {
        let tmp = root();
        let path = tmp.path().to_path_buf();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || create_agent_folder(&path, "racer").is_ok())
            })
            .collect();
        let won = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(won, 1);
    }

    #[test]
    fn a_slash_in_the_name_stays_one_folder() {
        assert_eq!(folder_name_for("team/notes"), "team-notes");
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "team/notes").unwrap();
        assert_eq!(created.path.file_name().unwrap(), "team-notes");
    }

    #[test]
    fn removing_deletes_the_folder_and_everything_in_it() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir_all(created.path.join("a/b")).unwrap();
        std::fs::write(created.path.join("a/b/c.txt"), "x").unwrap();
        std::fs::write(created.path.join(".hidden"), "x").unwrap();
        assert_eq!(
            remove_created_folder(&created.path, created.identity),
            Ok(RemovedFolder::Removed)
        );
        assert!(!created.path.exists());
    }

    #[test]
    fn a_symbolic_link_inside_is_removed_as_a_link_and_its_target_kept() {
        let tmp = root();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), "keep").unwrap();
        let agents = tmp.path().join("agents");
        let created = create_agent_folder(&agents, "otter").unwrap();
        std::os::unix::fs::symlink(&outside, created.path.join("link-to-dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("precious.txt"),
            created.path.join("link-to-file"),
        )
        .unwrap();
        assert_eq!(
            remove_created_folder(&created.path, created.identity),
            Ok(RemovedFolder::Removed)
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("precious.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn a_folder_that_is_already_gone_is_reported_as_gone() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_dir(&created.path).unwrap();
        assert_eq!(
            remove_created_folder(&created.path, created.identity),
            Ok(RemovedFolder::AlreadyGone)
        );
        let gone_parent = tmp.path().join("no-such/otter");
        assert_eq!(
            remove_created_folder(&gone_parent, created.identity),
            Ok(RemovedFolder::AlreadyGone)
        );
    }

    #[test]
    fn a_folder_replaced_by_another_at_the_same_path_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        // Keep the original inode alive elsewhere so the replacement cannot
        // reuse its number.
        std::fs::rename(&created.path, tmp.path().join("moved-away")).unwrap();
        std::fs::create_dir(&created.path).unwrap();
        std::fs::write(created.path.join("someone-elses.txt"), "keep").unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(
            refused.contains("no longer the one dux created"),
            "{refused}"
        );
        assert!(created.path.join("someone-elses.txt").exists());
    }

    #[test]
    fn a_symbolic_link_put_at_the_path_is_kept_and_not_followed() {
        let tmp = root();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), "keep").unwrap();
        let agents = tmp.path().join("agents");
        let created = create_agent_folder(&agents, "otter").unwrap();
        std::fs::remove_dir(&created.path).unwrap();
        std::os::unix::fs::symlink(&outside, &created.path).unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(outside.join("precious.txt").exists());
        assert!(
            std::fs::symlink_metadata(&created.path)
                .unwrap()
                .is_symlink()
        );
    }

    #[test]
    fn an_agents_folder_turned_into_a_symbolic_link_is_not_followed() {
        let tmp = root();
        let agents = tmp.path().join("agents");
        let created = create_agent_folder(&agents, "otter").unwrap();
        std::fs::write(created.path.join("file.txt"), "x").unwrap();
        // The real agents folder moves away and a link to it takes its place:
        // the same directory is reachable at the recorded path only through
        // the link.
        let real = tmp.path().join("real-agents");
        std::fs::rename(&agents, &real).unwrap();
        std::os::unix::fs::symlink(&real, &agents).unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(refused.contains("symbolic link"), "{refused}");
        assert!(real.join("otter/file.txt").exists());
    }

    #[test]
    fn a_file_put_at_the_path_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_dir(&created.path).unwrap();
        std::fs::write(&created.path, "a file now").unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(refused.contains("no longer a folder"), "{refused}");
        assert!(created.path.is_file());
    }

    #[test]
    fn the_file_count_counts_files_at_every_depth_and_says_when_it_cannot() {
        let tmp = root();
        let folder = tmp.path().join("otter");
        assert_eq!(count_folder_files(&folder), FolderFiles::Missing);
        std::fs::create_dir_all(folder.join("a/b")).unwrap();
        assert_eq!(
            count_folder_files(&folder),
            FolderFiles::Counted { count: 0 }
        );
        std::fs::write(folder.join("one"), "").unwrap();
        std::fs::write(folder.join("a/b/two"), "").unwrap();
        std::os::unix::fs::symlink(tmp.path(), folder.join("a/link")).unwrap();
        assert_eq!(
            count_folder_files(&folder),
            FolderFiles::Counted { count: 3 }
        );
        std::fs::write(tmp.path().join("plain"), "").unwrap();
        assert_eq!(
            count_folder_files(&tmp.path().join("plain")),
            FolderFiles::Unreadable
        );
    }
}
