//! The folders dux creates for standalone agents, under the agents folder
//! (`[defaults] standalone_agents_directory`, `<dux folder>/agents` when unset).
//!
//! Creating one refuses a name that is already taken, atomically, and records
//! the new directory's identity: its device and inode numbers and a random
//! mark written into a marker file inside it. Removing one deletes it only
//! while the directory at its path still holds that mark, reached without
//! following a symbolic link and without entering another filesystem. Every
//! function here touches the filesystem and belongs on a worker or a blocking
//! pool.

use std::io::{Read as _, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use crate::model::{DirectoryIdentity, FolderMark};
use crate::status_text::StatusText;

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

/// The name a fresh marker file is written under before it replaces the old.
const MARKER_SWAP_NAME: &str = ".dux-agent-folder.new";

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
    /// Another operation holds the folder's path; the text says which.
    Held(StatusText),
    /// The folder could not be created; the text says why.
    Failed {
        /// The folder that was to be created.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },
}

/// What a create asks the rest of dux about the folder's path.
pub struct CreateChecks<'a> {
    /// Hold the path for this create before anything at it is touched, or
    /// say why it cannot be held.
    pub hold: &'a dyn Fn(&Path) -> Result<(), StatusText>,
    /// Whether nothing but this create holds the folder at the path or works
    /// in it.
    pub unused: &'a dyn Fn(&Path) -> bool,
}

impl std::fmt::Debug for CreateChecks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateChecks").finish_non_exhaustive()
    }
}

/// [`create_agent_folder_with`] with no hold, never taking over a folder an
/// earlier create left.
///
/// # Errors
///
/// As [`create_agent_folder_with`].
pub fn create_agent_folder(
    root: &Path,
    agent_name: &str,
) -> Result<CreatedAgentFolder, CreateAgentFolderError> {
    create_agent_folder_with(
        root,
        agent_name,
        &CreateChecks {
            hold: &|_| Ok(()),
            unused: &|_| false,
        },
    )
}

/// Create the folder for a new standalone agent called `agent_name` inside
/// `root`, creating `root` first when it is missing.
///
/// The folder itself is created with one `mkdirat` relative to the opened
/// agents folder, so a folder that is already there, or one created by anyone
/// else at the same moment, is refused rather than shared. The one exception
/// is a folder an earlier create made and left before any agent was recorded
/// in it (see [`take_over_left_folder`]).
///
/// # Errors
///
/// [`CreateAgentFolderError::AlreadyThere`] when the name is taken,
/// [`CreateAgentFolderError::Held`] when `checks` cannot hold the path, and
/// [`CreateAgentFolderError::Failed`] when the agents folder or the new folder
/// cannot be created or read.
pub fn create_agent_folder_with(
    root: &Path,
    agent_name: &str,
    checks: &CreateChecks<'_>,
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
    (checks.hold)(&path).map_err(CreateAgentFolderError::Held)?;
    match rustix::fs::mkdirat(&root_fd, name.as_str(), Mode::from_bits_truncate(0o777)) {
        Ok(()) => {}
        Err(rustix::io::Errno::EXIST) => {
            return match take_over_left_folder(&root_fd, name.as_str(), &path, checks.unused) {
                Some(identity) => Ok(CreatedAgentFolder { path, identity }),
                None => Err(CreateAgentFolderError::AlreadyThere(path)),
            };
        }
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
    let mark = write_marker(&dir, MARKER_FILE_NAME)?;
    Ok(DirectoryIdentity {
        mark: Some(mark),
        ..numbers_of(&stat)
    })
}

/// Write a marker file called `file_name` with a fresh mark into `dir`.
fn write_marker(dir: &OwnedFd, file_name: &str) -> std::io::Result<FolderMark> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| std::io::Error::other(err.to_string()))?;
    let mark = FolderMark(bytes);
    let marker = rustix::fs::openat(
        dir,
        file_name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o644),
    )?;
    let mut marker = std::fs::File::from(marker);
    marker.write_all(format!("{MARKER_TEXT}{MARK_PREFIX}{}\n", mark.to_hex()).as_bytes())?;
    marker.sync_all()?;
    Ok(mark)
}

/// Whether a create of `agent_name` in `root` would go ahead with what is at
/// its folder's path, asking `unused` as the create does. A folder that
/// cannot be read is not reported as taken: the create says what is wrong.
pub fn new_folder_is_taken(root: &Path, agent_name: &str, unused: &dyn Fn(&Path) -> bool) -> bool {
    let name = folder_name_for(agent_name);
    if name.is_empty() || name == "." || name == ".." {
        return true;
    }
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let Ok(root_fd) = rustix::fs::open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    match rustix::fs::statat(&root_fd, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => left_by_a_create(&root_fd, name.as_str(), &root.join(&name), unused).is_none(),
        Err(_) => false,
    }
}

/// The folder at `name`, opened, when it is one of this user's holding only a
/// marked marker file, left by a create that never recorded its agent, and
/// `unused` says nothing uses it.
fn left_by_a_create(
    root_fd: &OwnedFd,
    name: &str,
    path: &Path,
    unused: &dyn Fn(&Path) -> bool,
) -> Option<(OwnedFd, rustix::fs::Stat)> {
    let dir = rustix::fs::openat(
        root_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?;
    let stat = rustix::fs::fstat(&dir).ok()?;
    if stat.st_uid != rustix::process::geteuid().as_raw()
        || filesystem_of(&dir)
            .ok()?
            .is_on(filesystem_of(root_fd).ok()?)
            != Some(true)
    {
        return None;
    }
    let names = entry_names(&dir).ok()?;
    if names.len() != 1 || names[0].to_bytes() != MARKER_FILE_NAME.as_bytes() {
        return None;
    }
    let MarkerFile::Holds(Some(_)) = read_marker(&dir) else {
        return None;
    };
    if !unused(path) {
        return None;
    }
    Some((dir, stat))
}

/// Take over a folder [`left_by_a_create`]; the fresh mark makes a removal
/// still holding the old one keep it.
fn take_over_left_folder(
    root_fd: &OwnedFd,
    name: &str,
    path: &Path,
    unused: &dyn Fn(&Path) -> bool,
) -> Option<DirectoryIdentity> {
    let (dir, stat) = left_by_a_create(root_fd, name, path, unused)?;
    let mark = write_marker(&dir, MARKER_SWAP_NAME).ok()?;
    if rustix::fs::renameat(&dir, MARKER_SWAP_NAME, &dir, MARKER_FILE_NAME).is_err() {
        let _ = rustix::fs::unlinkat(&dir, MARKER_SWAP_NAME, AtFlags::empty());
        return None;
    }
    crate::logger::info(&format!(
        "took over the folder {}, which an earlier create left holding only its marker file",
        path.display()
    ));
    Some(DirectoryIdentity {
        mark: Some(mark),
        ..numbers_of(&stat)
    })
}

/// Remove a folder dux created moments ago for an agent that never came up,
/// while it still holds nothing but its marker file and is still the folder
/// dux created. A folder anything was written into is kept and let go of,
/// as [`let_go`] does.
pub fn remove_if_unused(path: &Path, identity: DirectoryIdentity) {
    let kept = |why: &str| {
        crate::logger::warn(&format!(
            "kept the folder {} dux created for an agent that did not start: {why}",
            path.display()
        ));
    };
    let opened = match open_created_folder(path, identity.mark, &|_, dir| filesystem_of(dir)) {
        Ok(Some(opened)) => opened,
        Ok(None) => return,
        Err(why) => return kept(&why),
    };
    match entry_names(&opened.dir) {
        Ok(names)
            if names
                .iter()
                .all(|n| n.to_bytes() == MARKER_FILE_NAME.as_bytes()) => {}
        Ok(_) => {
            kept("something was written into it, so it is no longer dux's to delete");
            if let Err(err) = ignore_missing(rustix::fs::unlinkat(
                &opened.dir,
                MARKER_FILE_NAME,
                AtFlags::empty(),
            )) {
                kept(&format!("its marker file could not be removed ({err})"));
            }
            return;
        }
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
    /// Nothing was at the folder's path any more; it may have been moved.
    AlreadyGone,
}

/// Why a folder dux created was kept, in whole or in part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderKept {
    /// Whether anything in it was deleted before dux stopped.
    pub deleted_some: bool,
    /// Why, as the end of a sentence about the folder.
    pub reason: StatusText,
}

/// Whether a deletion removed anything before it stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeletionProgress {
    /// Nothing was deleted.
    Untouched,
    /// Something was deleted.
    Partial,
}

impl FolderKept {
    /// Kept whole, for `reason`.
    pub fn untouched(reason: impl Into<StatusText>) -> Self {
        Self {
            deleted_some: false,
            reason: reason.into(),
        }
    }

    /// The deletion of `folder` stopped at `stop`; `unrestored` names the
    /// directories whose mode could not be given back.
    fn stopped(
        folder: &Path,
        stop: &Stopped,
        progress: DeletionProgress,
        unrestored: &[(PathBuf, rustix::io::Errno)],
    ) -> Self {
        let deleted_some = progress == DeletionProgress::Partial;
        let why = match stop.why {
            StopWhy::Mounted => {
                "another filesystem is mounted there, and dux never deletes inside one".to_string()
            }
            StopWhy::CannotTellMounted => CANNOT_TELL_MOUNTED.to_string(),
            StopWhy::Failed(rustix::io::Errno::ACCESS | rustix::io::Errno::PERM) => {
                "dux does not have permission to delete it".to_string()
            }
            StopWhy::Failed(rustix::io::Errno::NOTEMPTY) => {
                "something was written into it while dux was deleting".to_string()
            }
            StopWhy::Failed(err) => format!("of an error ({err})"),
        };
        let mut reason = if stop.at.as_os_str().is_empty() {
            crate::status_text![format!(
                "everything in it was deleted but the folder itself was not, because {why}"
            )]
        } else if deleted_some {
            let left = match count_folder_files(folder) {
                FolderFiles::Counted { count: 0 } => "no files are left in it".to_string(),
                FolderFiles::Counted { count: 1 } => "1 file is still in it".to_string(),
                FolderFiles::Counted { count } => format!("{count} files are still in it"),
                FolderFiles::Missing => "nothing is left at its path".to_string(),
                FolderFiles::Unreadable => "dux could not count what is still in it".to_string(),
            };
            crate::status_text![
                "it stopped at ",
                q(stop.at.display()),
                format!(", because {why}, and {left}")
            ]
        } else {
            crate::status_text![
                "dux could not delete ",
                q(stop.at.display()),
                format!(", because {why}, so nothing in it was deleted")
            ]
        };
        for (at, err) in unrestored {
            if at.as_os_str().is_empty() {
                reason.push(", and dux could not give the folder itself its permissions back");
            } else {
                reason.push(", and dux could not give ");
                reason.push_quoted(at.display());
                reason.push(" its permissions back");
            }
            reason.push(format!(
                " after making it writable to delete inside it ({err})"
            ));
        }
        Self {
            deleted_some,
            reason,
        }
    }
}

impl From<StatusText> for FolderKept {
    fn from(reason: StatusText) -> Self {
        Self::untouched(reason)
    }
}

impl From<String> for FolderKept {
    fn from(reason: String) -> Self {
        Self::untouched(reason)
    }
}

/// Delete the folder dux created at `path`, and everything in it, while the
/// directory there still holds `identity`'s mark. No symbolic link is
/// followed and no other filesystem is entered, and a read-only directory of
/// this user's inside it is made writable first. The marker file goes last.
///
/// # Errors
///
/// Why the folder was kept, or where the deletion stopped and what is left.
pub fn remove_created_folder(
    path: &Path,
    identity: DirectoryIdentity,
) -> Result<RemovedFolder, FolderKept> {
    remove_created_folder_with(path, identity, &|_, dir| filesystem_of(dir))
}

/// [`remove_created_folder`], asking `filesystem_of` where each directory is,
/// by its path inside the folder (`..` for the folder it is in).
fn remove_created_folder_with(
    path: &Path,
    identity: DirectoryIdentity,
    filesystem_of: FilesystemOf<'_>,
) -> Result<RemovedFolder, FolderKept> {
    let Some(opened) = open_created_folder(path, identity.mark, filesystem_of)? else {
        return Ok(RemovedFolder::AlreadyGone);
    };
    let walk = Walk {
        filesystem: opened.filesystem,
        filesystem_of,
        uid: rustix::process::geteuid().as_raw(),
        deleted_some: std::cell::Cell::new(false),
        unrestored: std::cell::RefCell::new(Vec::new()),
    };
    let top_mode = walk.make_writable(&opened.dir);
    let stopped = |stop: Stopped| {
        walk.restore(&opened.dir, top_mode, Path::new(""));
        FolderKept::stopped(
            path,
            &stop,
            if walk.deleted_some.get() {
                DeletionProgress::Partial
            } else {
                DeletionProgress::Untouched
            },
            &walk.unrestored.borrow(),
        )
    };
    walk.remove_contents(&opened.dir, Path::new(""), Some(MARKER_FILE_NAME))
        .map_err(stopped)?;
    ignore_missing(rustix::fs::unlinkat(
        &opened.dir,
        MARKER_FILE_NAME,
        AtFlags::empty(),
    ))
    .map_err(|err| {
        stopped(Stopped {
            at: PathBuf::from(MARKER_FILE_NAME),
            why: StopWhy::Failed(err),
        })
    })?;
    walk.deleted_some.set(true);
    match rustix::fs::unlinkat(&opened.parent, opened.name.as_os_str(), AtFlags::REMOVEDIR) {
        Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(RemovedFolder::Removed),
        Err(err) => Err(stopped(Stopped {
            at: PathBuf::new(),
            why: StopWhy::Failed(err),
        })),
    }
}

/// Let go of the folder dux created at `path`: remove its marker file while
/// it still holds `mark`, so nothing in the folder says dux may delete it.
/// From then on the folder is the person's. A failure is logged.
pub fn let_go(path: &Path, mark: Option<FolderMark>) {
    let opened = match open_created_folder(path, mark, &|_, dir| filesystem_of(dir)) {
        Ok(Some(opened)) => opened,
        Ok(None) => return,
        Err(why) => {
            crate::logger::info(&format!(
                "left the marker file of {} as it is: {why}",
                path.display()
            ));
            return;
        }
    };
    if let Err(err) = ignore_missing(rustix::fs::unlinkat(
        &opened.dir,
        MARKER_FILE_NAME,
        AtFlags::empty(),
    )) {
        crate::logger::warn(&format!(
            "could not remove the marker file of {}, which dux no longer looks after: {err}",
            path.display()
        ));
    }
}

/// The filesystem a directory is on: its device and what the system says of
/// the mount it belongs to, which tells a bind mount of the same device apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Filesystem {
    device: u64,
    mount: Mount,
}

/// What the system says of the mount a directory belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mount {
    /// The mount's id.
    #[cfg(target_os = "linux")]
    Id(u64),
    /// Only whether a filesystem is mounted on the directory itself.
    #[cfg(target_os = "linux")]
    OnMountPoint(bool),
    /// Nothing, so whether a filesystem is mounted on it cannot be told.
    #[cfg(target_os = "linux")]
    Unknown,
    /// Nothing, and the device alone tells filesystems apart (macOS).
    #[cfg(not(target_os = "linux"))]
    DeviceOnly,
}

impl Filesystem {
    /// Whether this directory, directly inside one on `outer`, is on `outer`
    /// too; `None` when that cannot be told.
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            clippy::unnecessary_wraps,
            reason = "only Linux has a mount answer that cannot be told"
        )
    )]
    fn is_on(self, outer: Self) -> Option<bool> {
        if self.device != outer.device {
            return Some(false);
        }
        match (self.mount, outer.mount) {
            #[cfg(target_os = "linux")]
            (Mount::Id(id), Mount::Id(outer_id)) => Some(id == outer_id),
            #[cfg(target_os = "linux")]
            (Mount::OnMountPoint(mounted), _) => Some(!mounted),
            #[cfg(target_os = "linux")]
            _ => None,
            #[cfg(not(target_os = "linux"))]
            (Mount::DeviceOnly, Mount::DeviceOnly) => Some(true),
        }
    }
}

/// Which filesystem the opened directory at a path inside the folder is on.
type FilesystemOf<'a> = &'a dyn Fn(&Path, &OwnedFd) -> rustix::io::Result<Filesystem>;

fn filesystem_of(dir: &OwnedFd) -> rustix::io::Result<Filesystem> {
    Ok(Filesystem {
        device: numbers_of(&rustix::fs::fstat(dir)?).device,
        mount: mount_of(dir),
    })
}

/// The mount id where the kernel gives it (5.8 and later), else whether the
/// directory is a mount point by `/proc/self/mountinfo`.
#[cfg(target_os = "linux")]
fn mount_of(dir: &OwnedFd) -> Mount {
    if let Ok(found) =
        rustix::fs::statx(dir, "", AtFlags::EMPTY_PATH, rustix::fs::StatxFlags::MNT_ID)
        && found.stx_mask & rustix::fs::StatxFlags::MNT_ID.bits() != 0
    {
        return Mount::Id(found.stx_mnt_id);
    }
    mount_by_mountinfo(
        dir,
        std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .as_deref(),
    )
}

#[cfg(not(target_os = "linux"))]
fn mount_of(_dir: &OwnedFd) -> Mount {
    Mount::DeviceOnly
}

/// Whether `dir` is a mount point by the `mountinfo` text, found by the path
/// the kernel gives for the open directory.
#[cfg(target_os = "linux")]
fn mount_by_mountinfo(dir: &OwnedFd, mountinfo: Option<&str>) -> Mount {
    use rustix::fd::AsRawFd as _;
    let Some(mountinfo) = mountinfo else {
        return Mount::Unknown;
    };
    let Ok(path) = std::fs::read_link(format!("/proc/self/fd/{}", dir.as_raw_fd())) else {
        return Mount::Unknown;
    };
    // A directory removed while open, or one outside this process's root,
    // has no path a mount point could name.
    let bytes = path.as_os_str().as_bytes();
    if !bytes.starts_with(b"/") || bytes.ends_with(b" (deleted)") {
        return Mount::Unknown;
    }
    is_mount_point(mountinfo, bytes).map_or(Mount::Unknown, Mount::OnMountPoint)
}

/// Whether `path` is a mount point in the `mountinfo` text; `None` when a
/// line of it cannot be read.
#[cfg(target_os = "linux")]
fn is_mount_point(mountinfo: &str, path: &[u8]) -> Option<bool> {
    let mut found = false;
    for line in mountinfo.lines().filter(|line| !line.is_empty()) {
        let mount_point = unescape_mountinfo(line.split(' ').nth(4)?)?;
        found |= mount_point == path;
    }
    Some(found)
}

/// A `mountinfo` field with its `\ooo` octal escapes (space, tab, newline,
/// backslash) turned back into bytes.
#[cfg(target_os = "linux")]
fn unescape_mountinfo(field: &str) -> Option<Vec<u8>> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            let digits = std::str::from_utf8(bytes.get(at + 1..at + 4)?).ok()?;
            out.push(u8::from_str_radix(digits, 8).ok()?);
            at += 4;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    Some(out)
}

const CANNOT_TELL_MOUNTED: &str = "dux cannot tell on this system whether another filesystem is mounted in it, and dux never \
     deletes inside one";

/// The folder dux created, opened and checked, and the folder it is in.
struct OpenedFolder {
    parent: OwnedFd,
    dir: OwnedFd,
    name: std::ffi::OsString,
    filesystem: Filesystem,
}

/// Open the folder at `path` without following a link anywhere, and check it
/// is the directory dux created: a directory holding `mark`, on the same
/// filesystem as the folder it is in. `None` when nothing is there any more.
fn open_created_folder(
    path: &Path,
    mark: Option<FolderMark>,
    filesystem_of: FilesystemOf<'_>,
) -> Result<Option<OpenedFolder>, StatusText> {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err("its path does not name a folder inside another one".into());
    };
    match std::fs::canonicalize(parent) {
        Ok(resolved) if resolved == parent => {}
        Ok(resolved) => return Err(path_leads_elsewhere(parent, &resolved)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("dux could not read the folder it is in ({err})").into()),
    }
    let parent_fd = rustix::fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| StatusText::from(format!("dux could not open the folder it is in ({err})")))?;
    let stat = match rustix::fs::statat(&parent_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(err) => return Err(format!("dux could not read it ({err})").into()),
    };
    check_is_directory(&stat)?;
    let dir_fd = rustix::fs::openat(
        &parent_fd,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|err| StatusText::from(format!("dux could not open it ({err})")))?;
    let opened = rustix::fs::fstat(&dir_fd)
        .map_err(|err| StatusText::from(format!("dux could not read it ({err})")))?;
    check_is_directory(&opened)?;
    check_mark(&dir_fd, mark)?;
    let unreadable = |err| {
        StatusText::from(format!(
            "dux could not tell which filesystem it is on ({err})"
        ))
    };
    let filesystem = filesystem_of(Path::new(""), &dir_fd).map_err(unreadable)?;
    match filesystem.is_on(filesystem_of(Path::new(".."), &parent_fd).map_err(unreadable)?) {
        Some(true) => {}
        Some(false) => {
            return Err(
                "another filesystem is mounted on it, and dux never deletes inside one".into(),
            );
        }
        None => return Err(CANNOT_TELL_MOUNTED.into()),
    }
    Ok(Some(OpenedFolder {
        parent: parent_fd,
        dir: dir_fd,
        name: name.to_os_string(),
        filesystem,
    }))
}

/// Why a folder whose recorded `parent` now resolves to `resolved` is kept:
/// a link on the way to it, named, or a folder on the way renamed or moved.
fn path_leads_elsewhere(parent: &Path, resolved: &Path) -> StatusText {
    let mut walked = PathBuf::new();
    for part in parent.components() {
        walked.push(part);
        if let Ok(target) = std::fs::read_link(&walked) {
            return crate::status_text![
                q(walked.display()),
                " on the way to it is now a symbolic link to ",
                q(target.display()),
                ", so the folder may have moved there, and dux deletes only at the path it \
                 recorded, never through a link"
            ];
        }
    }
    crate::status_text![
        "the folders on the way to it now lead to ",
        q(resolved.display()),
        " instead of the path dux recorded, so one of them was renamed or moved, and dux \
         deletes only at the path it recorded"
    ]
}

/// Refuse anything at the folder's path other than a directory.
fn check_is_directory(stat: &rustix::fs::Stat) -> Result<(), String> {
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
    Ok(())
}

fn not_the_created_folder() -> String {
    "the folder at that path is no longer the one dux created: something else was put there since"
        .to_string()
}

/// Refuse a folder whose marker file does not hold `mark`.
fn check_mark(dir: &OwnedFd, mark: Option<FolderMark>) -> Result<(), String> {
    let Some(mark) = mark else {
        return Err(
            "dux has no mark on record for it, so it cannot be certain the folder there is \
             still the one it created"
                .to_string(),
        );
    };
    match read_marker(dir) {
        MarkerFile::Missing => Err(format!(
            "its {MARKER_FILE_NAME} file, which dux wrote into it to recognise it, is gone, so \
             dux cannot be certain it is still the folder it created"
        )),
        MarkerFile::Unreadable(err) => Err(format!(
            "dux could not read its {MARKER_FILE_NAME} file ({err})"
        )),
        MarkerFile::Holds(found) if found == Some(mark) => Ok(()),
        MarkerFile::Holds(_) => Err(not_the_created_folder()),
    }
}

/// What the marker file in a folder says.
enum MarkerFile {
    /// No plain file is there by that name.
    Missing,
    /// It could not be read.
    Unreadable(rustix::io::Errno),
    /// The mark it holds, `None` when it holds none dux can read.
    Holds(Option<FolderMark>),
}

/// Read the marker file in `dir` without following a link and only from a
/// plain file.
fn read_marker(dir: &OwnedFd) -> MarkerFile {
    let file = match rustix::fs::openat(
        dir,
        MARKER_FILE_NAME,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(file) => file,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => return MarkerFile::Missing,
        Err(err) => return MarkerFile::Unreadable(err),
    };
    match rustix::fs::fstat(&file) {
        Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile => {}
        Ok(_) => return MarkerFile::Missing,
        Err(err) => return MarkerFile::Unreadable(err),
    }
    let mut text = String::new();
    if std::fs::File::from(file)
        .take(MARKER_READ_LIMIT)
        .read_to_string(&mut text)
        .is_err()
    {
        return MarkerFile::Holds(None);
    }
    MarkerFile::Holds(
        text.lines()
            .find_map(|line| line.strip_prefix(MARK_PREFIX))
            .and_then(|hex| FolderMark::from_hex(hex.trim())),
    )
}

/// The names of the entries in the opened directory, without `.` and `..`.
fn entry_names(dir: &OwnedFd) -> rustix::io::Result<Vec<std::ffi::CString>> {
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

/// Where a deletion stopped, inside the folder, and why.
struct Stopped {
    at: PathBuf,
    why: StopWhy,
}

enum StopWhy {
    /// Another filesystem is mounted there.
    Mounted,
    /// Whether another filesystem is mounted there cannot be told.
    CannotTellMounted,
    Failed(rustix::io::Errno),
}

/// One deletion of the contents of a folder dux created.
struct Walk<'a> {
    /// The filesystem the folder is on, which the deletion never leaves.
    filesystem: Filesystem,
    filesystem_of: FilesystemOf<'a>,
    /// This user, whose read-only directories are made writable.
    uid: u32,
    deleted_some: std::cell::Cell<bool>,
    /// The directories whose mode could not be given back, and why.
    unrestored: std::cell::RefCell<Vec<(PathBuf, rustix::io::Errno)>>,
}

impl Walk<'_> {
    /// Delete everything inside the opened directory at `at` but `keep`,
    /// stopping at the first entry that cannot be deleted.
    fn remove_contents(&self, dir: &OwnedFd, at: &Path, keep: Option<&str>) -> Result<(), Stopped> {
        let failed = |at: &Path| {
            let at = at.to_path_buf();
            move |err| Stopped {
                at,
                why: StopWhy::Failed(err),
            }
        };
        let names = entry_names(dir)
            .map_err(failed(at))?
            .into_iter()
            .filter(|name| keep.is_none_or(|keep| name.to_bytes() != keep.as_bytes()));
        for name in names {
            let child_at = at.join(std::ffi::OsStr::from_bytes(name.to_bytes()));
            let stat = match rustix::fs::statat(dir, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(rustix::io::Errno::NOENT) => continue,
                Err(err) => return Err(failed(&child_at)(err)),
            };
            if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
                let child = rustix::fs::openat(
                    dir,
                    name.as_c_str(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(failed(&child_at))?;
                let on = (self.filesystem_of)(&child_at, &child)
                    .map_err(failed(&child_at))?
                    .is_on(self.filesystem);
                if on != Some(true) {
                    return Err(Stopped {
                        at: child_at,
                        why: on.map_or(StopWhy::CannotTellMounted, |_| StopWhy::Mounted),
                    });
                }
                let mode = self.make_writable(&child);
                let removed = self
                    .remove_contents(&child, &child_at, None)
                    .and_then(|()| {
                        ignore_missing(rustix::fs::unlinkat(
                            dir,
                            name.as_c_str(),
                            AtFlags::REMOVEDIR,
                        ))
                        .map_err(failed(&child_at))
                    });
                if removed.is_err() {
                    self.restore(&child, mode, &child_at);
                }
                removed?;
            } else {
                ignore_missing(rustix::fs::unlinkat(dir, name.as_c_str(), AtFlags::empty()))
                    .map_err(failed(&child_at))?;
            }
            self.deleted_some.set(true);
        }
        Ok(())
    }

    /// Give this user write and search permission on a directory of theirs,
    /// answering the mode it had when it was changed; a failure shows as the
    /// deletion inside it failing.
    fn make_writable(&self, dir: &OwnedFd) -> Option<Mode> {
        let stat = rustix::fs::fstat(dir).ok()?;
        let mode = Mode::from_raw_mode(stat.st_mode);
        if stat.st_uid == self.uid && !mode.contains(Mode::RWXU) {
            rustix::fs::fchmod(dir, mode | Mode::RWXU).ok()?;
            return Some(mode);
        }
        None
    }

    /// Give a directory that was not deleted the mode [`Self::make_writable`]
    /// found on it.
    fn restore(&self, dir: &OwnedFd, mode: Option<Mode>, at: &Path) {
        if let Some(mode) = mode
            && let Err(err) = rustix::fs::fchmod(dir, mode)
        {
            self.unrestored.borrow_mut().push((at.to_path_buf(), err));
        }
    }
}

fn ignore_missing(result: rustix::io::Result<()>) -> rustix::io::Result<()> {
    match result {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        other => other,
    }
}

/// The device and inode numbers in `stat`, with no mark.
fn numbers_of(stat: &rustix::fs::Stat) -> DirectoryIdentity {
    DirectoryIdentity {
        device: stat.st_dev.number(),
        inode: stat.st_ino.number(),
        mark: None,
    }
}

/// A `stat` field as the `u64` that `std`'s `MetadataExt` gives for it. The
/// field's type differs by target (`st_dev` is a signed 32-bit integer on
/// macOS and `u64` on Linux), and a signed value is sign-extended, so its bit
/// pattern is kept and one device always gives one number.
trait StatNumber {
    fn number(self) -> u64;
}

impl StatNumber for u64 {
    fn number(self) -> u64 {
        self
    }
}

impl StatNumber for u32 {
    fn number(self) -> u64 {
        u64::from(self)
    }
}

impl StatNumber for i32 {
    fn number(self) -> u64 {
        i64::from(self).cast_unsigned()
    }
}

impl StatNumber for i64 {
    fn number(self) -> u64 {
        self.cast_unsigned()
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

/// Count the files in `path` that deleting it would delete: never following a
/// symbolic link nor entering another filesystem.
pub fn count_folder_files(path: &Path) -> FolderFiles {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return FolderFiles::Missing,
        Ok(_) | Err(_) => return FolderFiles::Unreadable,
    }
    let mut count = 0_u64;
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .same_file_system(true)
        .min_depth(1)
    {
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

    fn refusal(path: &Path, identity: DirectoryIdentity) -> String {
        remove_created_folder(path, identity)
            .unwrap_err()
            .reason
            .message()
            .to_string()
    }

    fn marker_mark(folder: &Path) -> Option<FolderMark> {
        let text = std::fs::read_to_string(folder.join(MARKER_FILE_NAME)).ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix(MARK_PREFIX))
            .and_then(FolderMark::from_hex)
    }

    /// Stands in for a filesystem mounted on the directories `mounted` names:
    /// each reports another device than the one it is really on.
    fn mounted_at(mounted: &[&str]) -> impl Fn(&Path, &OwnedFd) -> rustix::io::Result<Filesystem> {
        let mounted: Vec<PathBuf> = mounted.iter().map(PathBuf::from).collect();
        move |at, dir| {
            let mut found = filesystem_of(dir)?;
            if mounted.iter().any(|m| m == at) {
                found.device ^= 1;
            }
            Ok(found)
        }
    }

    #[test]
    fn the_deletion_never_enters_a_directory_on_another_filesystem() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir_all(created.path.join("deep/mnt")).unwrap();
        std::fs::write(created.path.join("deep/mnt/theirs.txt"), "keep").unwrap();
        let kept =
            remove_created_folder_with(&created.path, created.identity, &mounted_at(&["deep/mnt"]))
                .unwrap_err();
        let text = kept.reason.message();
        assert!(text.contains("\"deep/mnt\""), "{text}");
        assert!(
            text.contains("another filesystem is mounted there"),
            "{text}"
        );
        assert!(created.path.join("deep/mnt/theirs.txt").exists());
        assert!(created.path.join(MARKER_FILE_NAME).exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_mount_reads_as_another_filesystem() {
        let open = |path: &str| {
            rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap()
        };
        let tmp = root();
        let inside = tmp.path().join("inside");
        std::fs::create_dir(&inside).unwrap();
        assert_eq!(
            filesystem_of(&open(tmp.path().to_str().unwrap())).unwrap(),
            filesystem_of(&open(inside.to_str().unwrap())).unwrap()
        );
        assert_ne!(
            filesystem_of(&open("/")).unwrap(),
            filesystem_of(&open("/proc")).unwrap()
        );
    }

    /// Stands in for a kernel with no mount ids: every directory answers
    /// `mount`, and those `mounted` names answer that a filesystem is mounted
    /// on them.
    #[cfg(target_os = "linux")]
    fn answering(
        mount: Mount,
        mounted: &[&str],
    ) -> impl Fn(&Path, &OwnedFd) -> rustix::io::Result<Filesystem> {
        let mounted: Vec<PathBuf> = mounted.iter().map(PathBuf::from).collect();
        move |at, dir| {
            Ok(Filesystem {
                device: filesystem_of(dir)?.device,
                mount: if mounted.iter().any(|m| m == at) {
                    Mount::OnMountPoint(true)
                } else {
                    mount
                },
            })
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn with_only_mount_points_known_the_deletion_stops_at_one() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir_all(created.path.join("deep/mnt")).unwrap();
        std::fs::write(created.path.join("deep/mnt/theirs.txt"), "keep").unwrap();
        let kept = remove_created_folder_with(
            &created.path,
            created.identity,
            &answering(Mount::OnMountPoint(false), &["deep/mnt"]),
        )
        .unwrap_err();
        let text = kept.reason.message();
        assert!(text.contains("\"deep/mnt\""), "{text}");
        assert!(
            text.contains("another filesystem is mounted there"),
            "{text}"
        );
        assert!(created.path.join("deep/mnt/theirs.txt").exists());

        let created = create_agent_folder(tmp.path(), "badger").unwrap();
        std::fs::create_dir_all(created.path.join("deep/plain")).unwrap();
        assert_eq!(
            remove_created_folder_with(
                &created.path,
                created.identity,
                &answering(Mount::OnMountPoint(false), &[]),
            ),
            Ok(RemovedFolder::Removed)
        );
        assert!(!created.path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn when_mounts_cannot_be_told_the_folder_is_kept_whole() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir(created.path.join("inside")).unwrap();
        std::fs::write(created.path.join("theirs.txt"), "keep").unwrap();
        let kept = remove_created_folder_with(
            &created.path,
            created.identity,
            &answering(Mount::Unknown, &[]),
        )
        .unwrap_err();
        assert!(!kept.deleted_some);
        assert!(
            kept.reason.message().contains(
                "dux cannot tell on this system whether another filesystem is mounted in it"
            ),
            "{kept:?}"
        );
        assert!(created.path.join("theirs.txt").exists());
        assert!(created.path.join("inside").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_subdirectory_whose_mount_cannot_be_told_is_not_entered() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir(created.path.join("inside")).unwrap();
        std::fs::write(created.path.join("inside/theirs.txt"), "keep").unwrap();
        let unknown_inside = |at: &Path, dir: &OwnedFd| {
            Ok(Filesystem {
                device: filesystem_of(dir)?.device,
                mount: if at == Path::new("inside") {
                    Mount::Unknown
                } else {
                    Mount::OnMountPoint(false)
                },
            })
        };
        let kept = remove_created_folder_with(&created.path, created.identity, &unknown_inside)
            .unwrap_err();
        let text = kept.reason.message();
        assert!(text.contains("\"inside\""), "{text}");
        assert!(text.contains("dux cannot tell on this system"), "{text}");
        assert!(created.path.join("inside/theirs.txt").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_says_which_paths_are_mount_points() {
        let text = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
36 22 0:5 / /proc rw,nosuid shared:5 - proc proc rw
40 22 8:2 /src /home/me/my\\040agents/mnt rw shared:1 - ext4 /dev/sda2 rw
";
        assert_eq!(is_mount_point(text, b"/proc"), Some(true));
        assert_eq!(is_mount_point(text, b"/home/me/my agents/mnt"), Some(true));
        assert_eq!(is_mount_point(text, b"/home/me/my agents"), Some(false));
        assert_eq!(
            is_mount_point(text, b"/home/me/my\\040agents/mnt"),
            Some(false)
        );
        assert_eq!(is_mount_point("22 1 8:2 /\n", b"/"), None);
        assert_eq!(is_mount_point("40 22 8:2 / /a\\04 rw\n", b"/a"), None);
    }

    /// The fallback a kernel without mount ids takes, run on this one.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_mountinfo_fallback_reads_a_real_mount_point_and_an_ordinary_folder() {
        let open = |path: &Path| {
            rustix::fs::open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .unwrap()
        };
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap();
        let tmp = root();
        let inside = tmp.path().join("in side");
        std::fs::create_dir(&inside).unwrap();
        assert_eq!(
            mount_by_mountinfo(&open(&inside), Some(&mountinfo)),
            Mount::OnMountPoint(false)
        );
        assert_eq!(
            mount_by_mountinfo(&open(Path::new("/proc")), Some(&mountinfo)),
            Mount::OnMountPoint(true)
        );
        assert_eq!(mount_by_mountinfo(&open(&inside), None), Mount::Unknown);
    }

    #[test]
    fn a_folder_with_a_filesystem_mounted_on_it_is_kept_whole() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::write(created.path.join("theirs.txt"), "keep").unwrap();
        let kept =
            remove_created_folder_with(&created.path, created.identity, &mounted_at(&[".."]))
                .unwrap_err();
        assert!(!kept.deleted_some);
        assert!(
            kept.reason
                .message()
                .contains("another filesystem is mounted on it"),
            "{kept:?}"
        );
        assert!(created.path.join("theirs.txt").exists());
    }

    /// Whichever of two folders the deletion reaches second stands in for a
    /// mount, so the first is deleted before it stops.
    #[test]
    fn a_deletion_that_stops_part_way_names_where_and_counts_what_is_left() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        for dir in ["one", "two"] {
            std::fs::create_dir(created.path.join(dir)).unwrap();
            std::fs::write(created.path.join(dir).join("file.txt"), "x").unwrap();
        }
        let seen = std::cell::Cell::new(0);
        let second_is_mounted = |_: &Path, dir: &OwnedFd| {
            let mut found = filesystem_of(dir)?;
            if dir_is_a_child(dir, &created.path) {
                seen.set(seen.get() + 1);
                if seen.get() == 2 {
                    found.device ^= 1;
                }
            }
            Ok(found)
        };
        let kept = remove_created_folder_with(&created.path, created.identity, &second_is_mounted)
            .unwrap_err();
        assert!(kept.deleted_some);
        let text = kept.reason.message();
        assert!(text.starts_with("it stopped at \""), "{text}");
        assert!(text.ends_with("and 1 file is still in it"), "{text}");
        let left: Vec<_> = ["one", "two"]
            .into_iter()
            .filter(|dir| created.path.join(dir).join("file.txt").exists())
            .collect();
        assert_eq!(left.len(), 1, "{left:?}");
        assert!(text.contains(&format!("\"{}\"", left[0])), "{text}");
    }

    /// Whether the opened directory sits directly inside `folder`.
    fn dir_is_a_child(dir: &OwnedFd, folder: &Path) -> bool {
        let inode = rustix::fs::fstat(dir).unwrap().st_ino;
        std::fs::read_dir(folder).unwrap().any(|entry| {
            use std::os::unix::fs::MetadataExt as _;
            entry.unwrap().metadata().unwrap().ino() == inode
        })
    }

    #[test]
    fn a_directory_dux_may_not_open_stops_the_deletion_and_is_named() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        let locked = created.path.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("inner.txt"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let kept = remove_created_folder(&created.path, created.identity).unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!kept.deleted_some);
        assert_eq!(
            kept.reason.message(),
            "dux could not delete \"locked\", because dux does not have permission to delete \
             it, so nothing in it was deleted"
        );
        assert!(locked.join("inner.txt").exists());
    }

    #[test]
    fn letting_go_removes_only_a_marker_file_that_holds_the_mark() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::write(created.path.join("work.txt"), "keep").unwrap();
        let_go(&created.path, Some(FolderMark([7; 16])));
        assert!(created.path.join(MARKER_FILE_NAME).exists());
        let_go(&created.path, created.identity.mark);
        assert!(!created.path.join(MARKER_FILE_NAME).exists());
        assert!(created.path.join("work.txt").exists());
    }

    fn checks<'a>(
        hold: &'a dyn Fn(&Path) -> Result<(), StatusText>,
        unused: &'a dyn Fn(&Path) -> bool,
    ) -> CreateChecks<'a> {
        CreateChecks { hold, unused }
    }

    /// A folder an earlier create left holding only its marker file is taken
    /// over with a fresh mark.
    #[test]
    fn a_folder_left_holding_only_its_marker_file_is_taken_over() {
        let tmp = root();
        let left = create_agent_folder(tmp.path(), "otter").unwrap();
        let taken = create_agent_folder_with(tmp.path(), "otter", &checks(&|_| Ok(()), &|_| true))
            .expect("taken over");
        assert_eq!(taken.path, left.path);
        assert_ne!(taken.identity.mark, left.identity.mark);
        assert_eq!(marker_mark(&taken.path), taken.identity.mark);
        assert_eq!(
            std::fs::read_dir(&taken.path).unwrap().count(),
            1,
            "nothing but the marker file"
        );
    }

    #[test]
    fn a_left_folder_is_not_taken_over_when_anything_else_is_there_or_uses_it() {
        let tmp = root();
        let take = |name: &str, unused: bool| {
            create_agent_folder_with(tmp.path(), name, &checks(&|_| Ok(()), &move |_| unused))
        };
        let in_use = create_agent_folder(tmp.path(), "in-use").unwrap();
        assert!(matches!(
            take("in-use", false),
            Err(CreateAgentFolderError::AlreadyThere(_))
        ));
        let written = create_agent_folder(tmp.path(), "written").unwrap();
        std::fs::write(written.path.join("note"), "x").unwrap();
        let unmarked = tmp.path().join("unmarked");
        std::fs::create_dir(&unmarked).unwrap();
        std::fs::write(unmarked.join(MARKER_FILE_NAME), "not a mark\n").unwrap();
        for name in ["written", "unmarked"] {
            assert!(
                matches!(
                    take(name, true),
                    Err(CreateAgentFolderError::AlreadyThere(_))
                ),
                "{name}"
            );
        }
        assert_eq!(marker_mark(&in_use.path), in_use.identity.mark);
        assert_eq!(marker_mark(&written.path), written.identity.mark);
    }

    #[test]
    fn the_lookup_calls_taken_exactly_what_a_create_refuses() {
        let tmp = root();
        let left = create_agent_folder(tmp.path(), "left").unwrap();
        let written = create_agent_folder(tmp.path(), "written").unwrap();
        std::fs::write(written.path.join("note"), "x").unwrap();
        for (name, unused, taken) in [
            ("missing", true, false),
            ("left", false, true),
            ("written", true, true),
            ("left", true, false),
        ] {
            assert_eq!(
                new_folder_is_taken(tmp.path(), name, &move |_| unused),
                taken,
                "{name}"
            );
            let refused = matches!(
                create_agent_folder_with(tmp.path(), name, &checks(&|_| Ok(()), &move |_| unused)),
                Err(CreateAgentFolderError::AlreadyThere(_))
            );
            assert_eq!(refused, taken, "{name}");
        }
        assert!(left.path.is_dir());
    }

    #[test]
    fn a_create_whose_path_cannot_be_held_touches_nothing() {
        let tmp = root();
        let refused = create_agent_folder_with(
            tmp.path(),
            "otter",
            &checks(&|_| Err(StatusText::plain("held")), &|_| true),
        );
        assert!(matches!(refused, Err(CreateAgentFolderError::Held(_))));
        assert!(!tmp.path().join("otter").exists());
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
        let refused = refusal(&created.path, created.identity);
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
        let refused = refusal(&created.path, created.identity);
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
        let refused = refusal(&created.path, unmarked);
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
        assert!(
            !used.path.join(MARKER_FILE_NAME).exists(),
            "a kept folder is let go of"
        );

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
        let refused = refusal(&created.path, created.identity);
        assert!(refused.contains(MARKER_FILE_NAME), "{refused}");
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
        let message = refusal(&created.path, reused);
        assert!(message.contains(MARKER_FILE_NAME), "{message}");
        assert!(created.path.join("someone-elses.txt").exists());
    }

    /// A reboot can renumber the device and a restore from backup the inode
    /// too; the mark still says it is the folder dux created.
    #[test]
    fn a_folder_with_other_numbers_and_its_mark_is_deleted() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::write(created.path.join("work.txt"), "x").unwrap();
        let renumbered = DirectoryIdentity {
            device: created.identity.device ^ 1,
            inode: created.identity.inode ^ 1,
            ..created.identity
        };
        assert_eq!(
            remove_created_folder(&created.path, renumbered),
            Ok(RemovedFolder::Removed)
        );
        assert!(!created.path.exists());
    }

    #[test]
    fn a_truncated_marker_file_keeps_the_folder() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        let marker = created.path.join(MARKER_FILE_NAME);
        let text = std::fs::read_to_string(&marker).unwrap();
        std::fs::write(&marker, &text[..text.len() - 10]).unwrap();
        assert!(remove_created_folder(&created.path, created.identity).is_err());
        assert!(created.path.is_dir());
    }

    /// A module cache a package manager made read-only is deleted, as
    /// `rm -rf` alone cannot.
    #[test]
    fn a_read_only_directory_inside_is_deleted() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        let cache = created.path.join("pkg/mod/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("module.zip"), "x").unwrap();
        for dir in [&cache, &created.path.join("pkg/mod")] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        assert_eq!(
            remove_created_folder(&created.path, created.identity),
            Ok(RemovedFolder::Removed)
        );
        assert!(!created.path.exists());
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_deletion_that_stops_gives_read_only_directories_their_modes_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::create_dir_all(created.path.join("a/b")).unwrap();
        for dir in [created.path.join("a"), created.path.clone()] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let kept =
            remove_created_folder_with(&created.path, created.identity, &mounted_at(&["a/b"]))
                .unwrap_err();
        let modes = (mode_of(&created.path), mode_of(&created.path.join("a")));
        for dir in [created.path.clone(), created.path.join("a")] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(!kept.deleted_some, "{kept:?}");
        assert_eq!(modes, (0o555, 0o555));
    }

    #[test]
    fn a_folder_left_empty_but_not_deleted_gets_its_mode_back() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::write(created.path.join("file.txt"), "x").unwrap();
        std::fs::set_permissions(&created.path, std::fs::Permissions::from_mode(0o555)).unwrap();
        // The folder the agent's folder is in refuses the last step.
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let kept = remove_created_folder(&created.path, created.identity).unwrap_err();
        let mode = mode_of(&created.path);
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&created.path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(kept.deleted_some);
        assert_eq!(mode, 0o555);
    }

    #[test]
    fn a_mode_that_could_not_be_given_back_is_named() {
        let reason = FolderKept::stopped(
            Path::new("/nowhere/otter"),
            &Stopped {
                at: PathBuf::from("a/b"),
                why: StopWhy::Mounted,
            },
            DeletionProgress::Untouched,
            &[(PathBuf::from("a"), rustix::io::Errno::PERM)],
        )
        .reason;
        let text = reason.message();
        assert!(
            text.contains(
                ", so nothing in it was deleted, and dux could not give \"a\" its permissions \
                 back after making it writable to delete inside it ("
            ),
            "{text}"
        );
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
        let refused = refusal(&created.path, created.identity);
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
        let refused = refusal(&created.path, created.identity);
        assert_eq!(
            refused,
            format!(
                "\"{}\" on the way to it is now a symbolic link to \"{}\", so the folder may \
                 have moved there, and dux deletes only at the path it recorded, never through \
                 a link",
                agents.display(),
                real.display()
            )
        );
        assert!(real.join("otter/file.txt").exists());
    }

    #[test]
    fn a_path_that_now_leads_elsewhere_without_a_link_is_kept_and_says_so() {
        let tmp = root();
        let created = create_agent_folder(&tmp.path().join("agents"), "otter").unwrap();
        std::fs::create_dir(tmp.path().join("other")).unwrap();
        let roundabout = tmp.path().join("other/../agents/otter");
        let refused = refusal(&roundabout, created.identity);
        assert_eq!(
            refused,
            format!(
                "the folders on the way to it now lead to \"{}\" instead of the path dux \
                 recorded, so one of them was renamed or moved, and dux deletes only at the \
                 path it recorded",
                tmp.path().join("agents").display()
            )
        );
        assert!(created.path.is_dir());
    }

    #[test]
    fn a_file_put_at_the_path_is_kept() {
        let tmp = root();
        let created = create_agent_folder(tmp.path(), "otter").unwrap();
        std::fs::remove_dir_all(&created.path).unwrap();
        std::fs::write(&created.path, "a file now").unwrap();
        let refused = refusal(&created.path, created.identity);
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
