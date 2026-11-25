//! The folders dux creates for standalone agents, under the agents folder
//! (`[defaults] standalone_agents_directory`, `<dux folder>/agents` when unset).
//!
//! Creating one refuses a name that is already taken, atomically, and records
//! the new directory's identity: its device and inode numbers and a random
//! mark written into a marker file inside it. Removing one deletes it only
//! while the directory at its path still has all three, reached without
//! following a symbolic link anywhere inside it. Every function here touches
//! the filesystem and belongs on a worker or a blocking pool.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use crate::model::{DirectoryIdentity, FolderMark};

/// The hidden file dux writes into a folder it creates, holding the folder's
/// mark. Deleting the agent deletes the folder only while this file is there
/// and still holds that mark; without it the folder is kept.
pub const MARKER_FILE_NAME: &str = ".dux-agent-folder";

/// What the marker file says to a person who opens it, ahead of the mark.
const MARKER_TEXT: &str = "dux created this folder for a standalone agent. Deleting that agent \
in dux can delete this folder and everything in it, but only while this file is here and \
unchanged. Delete this file to keep the folder whatever happens to the agent.\n";

/// The line in the marker file that carries the mark.
const MARK_PREFIX: &str = "mark: ";

/// The most of a marker file dux reads.
const MARKER_READ_LIMIT: u64 = 4096;

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
    match mark_new_folder(&root_fd, name.as_str()) {
        Ok(identity) => Ok(CreatedAgentFolder { path, identity }),
        Err(err) => {
            // Only what this call made goes: the marker, then the folder,
            // which `unlinkat` removes only while it is empty.
            let _ = rustix::fs::openat(
                &root_fd,
                name.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .and_then(|dir| rustix::fs::unlinkat(&dir, MARKER_FILE_NAME, AtFlags::empty()));
            let _ = rustix::fs::unlinkat(&root_fd, name.as_str(), AtFlags::REMOVEDIR);
            Err(failed(
                &path,
                format!("dux could not write the file that marks it as dux's ({err})"),
            ))
        }
    }
}

/// Read the new folder's numbers and write its marker file with a fresh mark.
fn mark_new_folder(
    root_fd: &rustix::fd::OwnedFd,
    name: &str,
) -> std::io::Result<DirectoryIdentity> {
    let dir = rustix::fs::openat(
        root_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let stat = rustix::fs::fstat(&dir)?;
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| std::io::Error::other(err.to_string()))?;
    let mark = FolderMark(bytes);
    let marker = rustix::fs::openat(
        &dir,
        MARKER_FILE_NAME,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o644),
    )?;
    let mut marker = std::fs::File::from(marker);
    marker.write_all(format!("{MARKER_TEXT}{MARK_PREFIX}{}\n", mark.to_hex()).as_bytes())?;
    marker.sync_all()?;
    Ok(DirectoryIdentity {
        mark: Some(mark),
        ..numbers_of(&stat)
    })
}

/// Remove a folder dux created moments ago for an agent that never came up,
/// while it still holds nothing but its marker file and is still the folder
/// dux created. A folder anything was written into is kept.
pub fn remove_if_unused(path: &Path, identity: DirectoryIdentity) {
    let kept = |why: &str| {
        crate::logger::warn(&format!(
            "kept the folder {} dux created for an agent that did not start: {why}",
            path.display()
        ));
    };
    let opened = match open_created_folder(path, identity) {
        Ok(Some(opened)) => opened,
        Ok(None) => return,
        Err(why) => return kept(&why),
    };
    match entry_names(&opened.dir) {
        Ok(names)
            if names
                .iter()
                .all(|n| n.to_bytes() == MARKER_FILE_NAME.as_bytes()) => {}
        Ok(_) => return kept("something was written into it"),
        Err(err) => return kept(&err.to_string()),
    }
    let removed = ignore_missing(rustix::fs::unlinkat(
        &opened.dir,
        MARKER_FILE_NAME,
        AtFlags::empty(),
    ))
    .and_then(|()| {
        ignore_missing(rustix::fs::unlinkat(
            &opened.parent,
            opened.name.as_os_str(),
            AtFlags::REMOVEDIR,
        ))
    });
    if let Err(err) = removed {
        kept(&err.to_string());
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
/// directory there still has `identity`: the same device and inode numbers,
/// and a marker file holding the same mark.
///
/// Nothing is followed: a folder on the way to `path` that is now a symbolic
/// link, a symbolic link or a file at `path`, a directory there with other
/// numbers, or one whose marker file is missing, is not a plain file, or holds
/// another mark is refused and left alone, and so is any folder when
/// `identity` has no mark. Inside the folder a symbolic link is removed as a
/// link and never followed, and every entry is reached relative to the
/// directory that was checked. The marker file goes last, so a deletion that
/// stops part way leaves a folder dux still recognises.
///
/// # Errors
///
/// The sentence fragment saying why the folder was kept, or what stopped the
/// deletion part way through.
pub fn remove_created_folder(
    path: &Path,
    identity: DirectoryIdentity,
) -> Result<RemovedFolder, String> {
    let Some(opened) = open_created_folder(path, identity)? else {
        return Ok(RemovedFolder::AlreadyGone);
    };
    remove_contents(&opened.dir, Some(MARKER_FILE_NAME))
        .map_err(|err| format!("deleting what it holds stopped part way ({err})"))?;
    ignore_missing(rustix::fs::unlinkat(
        &opened.dir,
        MARKER_FILE_NAME,
        AtFlags::empty(),
    ))
    .map_err(|err| format!("deleting what it holds stopped part way ({err})"))?;
    match rustix::fs::unlinkat(&opened.parent, opened.name.as_os_str(), AtFlags::REMOVEDIR) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(RemovedFolder::Removed),
        Err(err) => Err(format!(
            "its contents were deleted but the folder itself was not ({err}); something may \
             have written into it meanwhile"
        )),
    }
}

/// The folder dux created, opened and checked, and the folder it is in.
struct OpenedFolder {
    parent: rustix::fd::OwnedFd,
    dir: rustix::fd::OwnedFd,
    name: std::ffi::OsString,
}

/// Open the folder at `path` without following a link anywhere, and check it
/// is the directory dux created. `None` when nothing is there any more.
fn open_created_folder(
    path: &Path,
    identity: DirectoryIdentity,
) -> Result<Option<OpenedFolder>, String> {
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(err) => return Err(format!("dux could not read it ({err})")),
    };
    check_numbers(&stat, identity)?;
    let dir_fd = rustix::fs::openat(
        &parent_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| format!("dux could not open it ({err})"))?;
    let opened =
        rustix::fs::fstat(&dir_fd).map_err(|err| format!("dux could not read it ({err})"))?;
    check_numbers(&opened, identity)?;
    check_mark(&dir_fd, identity.mark)?;
    Ok(Some(OpenedFolder {
        parent: parent_fd,
        dir: dir_fd,
        name: name.to_os_string(),
    }))
}

/// Refuse anything at the folder's path other than a directory with the
/// device and inode numbers dux recorded.
fn check_numbers(stat: &rustix::fs::Stat, identity: DirectoryIdentity) -> Result<(), String> {
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
    let numbers = numbers_of(stat);
    if (numbers.device, numbers.inode) != (identity.device, identity.inode) {
        return Err(not_the_created_folder());
    }
    Ok(())
}

fn not_the_created_folder() -> String {
    "the folder at that path is no longer the one dux created: something else was put there since"
        .to_string()
}

/// Refuse a folder whose marker file does not hold `mark`, read without
/// following a link and only from a plain file.
fn check_mark(dir: &rustix::fd::OwnedFd, mark: Option<FolderMark>) -> Result<(), String> {
    let Some(mark) = mark else {
        return Err(
            "dux has no mark on record for it, so it cannot be certain the folder there is \
             still the one it created"
                .to_string(),
        );
    };
    let missing = || {
        format!(
            "its {MARKER_FILE_NAME} file, which dux wrote into it to recognise it, is gone, so \
             dux cannot be certain it is still the folder it created"
        )
    };
    let file = match rustix::fs::openat(
        dir,
        MARKER_FILE_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => return Err(missing()),
        Err(err) => {
            return Err(format!(
                "dux could not read its {MARKER_FILE_NAME} file ({err})"
            ));
        }
    };
    let stat = rustix::fs::fstat(&file)
        .map_err(|err| format!("dux could not read its {MARKER_FILE_NAME} file ({err})"))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(missing());
    }
    let mut text = String::new();
    std::fs::File::from(file)
        .take(MARKER_READ_LIMIT)
        .read_to_string(&mut text)
        .map_err(|_| not_the_created_folder())?;
    let found = text
        .lines()
        .find_map(|line| line.strip_prefix(MARK_PREFIX))
        .and_then(|hex| FolderMark::from_hex(hex.trim()));
    if found == Some(mark) {
        Ok(())
    } else {
        Err(not_the_created_folder())
    }
}

/// The names of the entries in the opened directory, without `.` and `..`.
fn entry_names(dir: &rustix::fd::OwnedFd) -> rustix::io::Result<Vec<std::ffi::CString>> {
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
    Ok(names)
}

/// Delete everything inside the opened directory but `keep`, never following
/// a link.
fn remove_contents(dir: &rustix::fd::OwnedFd, keep: Option<&str>) -> rustix::io::Result<()> {
    let names = entry_names(dir)?
        .into_iter()
        .filter(|name| keep.is_none_or(|keep| name.to_bytes() != keep.as_bytes()));
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
            remove_contents(&child, None)?;
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

/// The device and inode numbers in `stat`, with no mark.
fn numbers_of(stat: &rustix::fs::Stat) -> DirectoryIdentity {
    // `st_dev` and `st_ino` are unsigned on every Unix dux runs on; the
    // conversion only widens.
    #[expect(
        clippy::useless_conversion,
        reason = "st_dev is u32 on some targets and u64 on others"
    )]
    DirectoryIdentity {
        device: u64::from(stat.st_dev),
        inode: u64::from(stat.st_ino),
        mark: None,
    }
}

/// How many files a folder holds, for the delete dialog's warning.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FolderFiles {
    /// The folder is there and holds `count` files (every entry that is not a
    /// folder, links included, at any depth, except the marker file dux keeps
    /// at its top).
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
            Ok(entry) if entry.depth() == 1 && entry.file_name() == MARKER_FILE_NAME => {}
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
        let numbers = DirectoryIdentity::of(&created.path).unwrap();
        assert_eq!(
            (created.identity.device, created.identity.inode),
            (numbers.device, numbers.inode)
        );
        let mark = created.identity.mark.expect("a mark");
        let marker = std::fs::read_to_string(created.path.join(MARKER_FILE_NAME)).unwrap();
        assert!(marker.contains(&mark.to_hex()), "{marker}");
        assert!(marker.starts_with("dux created this folder"), "{marker}");
        assert_eq!(
            count_folder_files(&created.path),
            FolderFiles::Counted { count: 0 }
        );
    }

    /// Two folders never get the same mark.
    #[test]
    fn every_created_folder_gets_its_own_mark() {
        let tmp = root();
        let one = create_agent_folder(tmp.path(), "one").unwrap();
        let two = create_agent_folder(tmp.path(), "two").unwrap();
        assert_ne!(one.identity.mark, two.identity.mark);
    }

    #[test]
    fn a_mark_reads_back_from_its_hex_and_nothing_else_does() {
        let mark = FolderMark([0xab; 16]);
        assert_eq!(FolderMark::from_hex(&mark.to_hex()), Some(mark));
        assert_eq!(FolderMark::from_hex("ab"), None);
        assert_eq!(FolderMark::from_hex(&"zz".repeat(16)), None);
    }

    /// A folder whose marker file was deleted is kept, because nothing then
    /// tells it apart from a folder made at the same path.
    #[test]
    fn a_folder_whose_marker_file_is_gone_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_file(created.path.join(MARKER_FILE_NAME)).unwrap();
        std::fs::write(created.path.join("work.txt"), "keep").unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(refused.contains(MARKER_FILE_NAME), "{refused}");
        assert!(created.path.join("work.txt").exists());
    }

    /// A marker file holding another mark, or one turned into a link to a
    /// file that holds the right one, is not the folder's.
    #[test]
    fn a_marker_file_with_another_mark_or_turned_into_a_link_is_not_trusted() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        let marker = created.path.join(MARKER_FILE_NAME);
        let real = std::fs::read_to_string(&marker).unwrap();
        std::fs::write(
            &marker,
            format!("{MARK_PREFIX}{}\n", FolderMark([7; 16]).to_hex()),
        )
        .unwrap();
        let refused = remove_created_folder(&created.path, created.identity).unwrap_err();
        assert!(
            refused.contains("no longer the one dux created"),
            "{refused}"
        );

        let elsewhere = tmp.path().join("copied-marker");
        std::fs::write(&elsewhere, real).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &marker).unwrap();
        assert!(remove_created_folder(&created.path, created.identity).is_err());
        assert!(created.path.is_dir());
    }

    /// A record with no mark keeps its folder, whatever is in it.
    #[test]
    fn a_folder_recorded_without_a_mark_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        let unmarked = DirectoryIdentity {
            mark: None,
            ..created.identity
        };
        let refused = remove_created_folder(&created.path, unmarked).unwrap_err();
        assert!(refused.contains("no mark on record"), "{refused}");
        assert!(created.path.is_dir());
    }

    /// A launch that failed takes back a folder that holds nothing but its
    /// marker file, and keeps one anything was written into or that is no
    /// longer the folder dux created.
    #[test]
    fn an_unused_folder_is_taken_back_and_a_used_or_replaced_one_kept() {
        let tmp = root();
        let unused = create_agent_folder(tmp.path(), "unused").unwrap();
        remove_if_unused(&unused.path, unused.identity);
        assert!(!unused.path.exists());

        let used = create_agent_folder(tmp.path(), "used").unwrap();
        std::fs::write(used.path.join("note"), "x").unwrap();
        remove_if_unused(&used.path, used.identity);
        assert!(used.path.join("note").exists());

        let replaced = create_agent_folder(tmp.path(), "replaced").unwrap();
        std::fs::remove_dir_all(&replaced.path).unwrap();
        std::fs::create_dir(&replaced.path).unwrap();
        remove_if_unused(&replaced.path, replaced.identity);
        assert!(replaced.path.is_dir());
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
        std::fs::remove_dir_all(&created.path).unwrap();
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

    /// The folder is deleted and a new one made at the same path, which on
    /// ext4 takes the freed inode number, so the device and inode match.
    #[test]
    fn a_folder_recreated_at_the_same_path_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_dir_all(&created.path).unwrap();
        std::fs::create_dir(&created.path).unwrap();
        std::fs::write(created.path.join("someone-elses.txt"), "keep").unwrap();
        assert!(remove_created_folder(&created.path, created.identity).is_err());
        assert!(created.path.join("someone-elses.txt").exists());
    }

    /// The same, with the new folder given the recorded device and inode
    /// numbers on every filesystem, as a filesystem that reuses them does.
    #[test]
    fn a_recreated_folder_with_the_recorded_inode_number_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_dir_all(&created.path).unwrap();
        std::fs::create_dir(&created.path).unwrap();
        std::fs::write(created.path.join("someone-elses.txt"), "keep").unwrap();
        let now = DirectoryIdentity::of(&created.path).unwrap();
        let reused = DirectoryIdentity {
            device: now.device,
            inode: now.inode,
            ..created.identity
        };
        let refused = remove_created_folder(&created.path, reused).unwrap_err();
        assert!(refused.contains(MARKER_FILE_NAME), "{refused}");
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
        std::fs::remove_dir_all(&created.path).unwrap();
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
        std::fs::remove_dir_all(&created.path).unwrap();
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
        // The marker file at the top is dux's own; one further down is not.
        std::fs::write(folder.join(MARKER_FILE_NAME), "").unwrap();
        std::fs::write(folder.join("a").join(MARKER_FILE_NAME), "").unwrap();
        assert_eq!(
            count_folder_files(&folder),
            FolderFiles::Counted { count: 4 }
        );
        std::fs::write(tmp.path().join("plain"), "").unwrap();
        assert_eq!(
            count_folder_files(&tmp.path().join("plain")),
            FolderFiles::Unreadable
        );
    }
}
