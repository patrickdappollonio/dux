//! Folding whole folders into one changed-files row.
//!
//! The listing runs `git status --untracked-files=normal`, which is git's own
//! default: a directory with nothing tracked inside it is reported once, as
//! the directory, rather than once per file. This module turns those entries
//! into [`ChangedFileKind::Directory`] rows with a file count, folds the same
//! folder on the staged side once it has been staged whole, and lists one
//! folder's contents a level at a time for a surface that expands it.

use super::*;
use crate::model::ChangedFileKind;

/// Which half of the changes panel a folder row belongs to. The two halves
/// answer "what is inside this folder" from different places: the working
/// tree for an untracked folder, the index for a staged one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangesSide {
    Staged,
    Unstaged,
}

/// What git says is inside one untracked folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum UntrackedFolder {
    /// Its untracked, not-ignored files, the repositories of their own it
    /// holds, and the lines its files hold between them (0 past the budget).
    Contents(crate::model::FolderContents, usize),
    /// The folder is a repository of its own, which git does not look inside.
    NestedRepository,
    /// The folder is a linked worktree of this same repository.
    LinkedWorktree,
}

impl UntrackedFolder {
    /// The lines the folder's files hold, as its row's additions: an
    /// untracked file is all additions.
    pub(super) fn additions(&self) -> usize {
        match self {
            Self::Contents(_, additions) => *additions,
            Self::NestedRepository | Self::LinkedWorktree => 0,
        }
    }

    pub(super) fn kind(self) -> ChangedFileKind {
        match self {
            Self::Contents(contents, _) => ChangedFileKind::Directory(contents),
            Self::NestedRepository => ChangedFileKind::NestedRepository,
            Self::LinkedWorktree => ChangedFileKind::LinkedWorktree,
        }
    }
}

/// One untracked folder's `ls-files` records, gathered while they are read.
#[derive(Default)]
struct Tally<'r> {
    /// The folder is itself a repository of its own.
    is_repository: bool,
    /// Every file record inside it, kept for the fingerprint.
    files: Vec<&'r [u8]>,
    /// Every repository record inside it (`path/`), at any depth: told apart
    /// into repositories of their own and worktrees of this repository once
    /// the tally is finished.
    repositories: Vec<&'r [u8]>,
}

impl<'r> Tally<'r> {
    /// Count one record found inside the folder. A record ending in `/` is a
    /// repository git did not look inside, and it is not a file.
    fn add(&mut self, record: &'r [u8]) {
        if record.ends_with(b"/") {
            self.repositories.push(record);
        } else {
            self.files.push(record);
        }
    }
}

/// Turn the tallies of one read into what each folder holds, fingerprinting
/// folders and counting their lines in order for as long as `budget` lasts.
/// It is the budget that already bounds the untracked files' line counts,
/// shared with them, so a listing reads no more files than it did before
/// folding. A folder that would not fit in what is left gets neither a
/// fingerprint nor line counts rather than partial ones, which could miss
/// exactly the file that changed.
///
/// `prefix` is what turns a tally's name into its worktree-relative path, so
/// a repository row can be told apart from a linked worktree of this same
/// repository (which asks git, but only for a directory whose `.git` is a
/// file).
fn finish_tallies(
    worktree: &Path,
    prefix: &str,
    tallies: Vec<(String, Tally<'_>)>,
    budget: &mut usize,
) -> Vec<(String, UntrackedFolder)> {
    // Read once per listing, and only if some folder is a repository.
    let mut common: Option<Option<PathBuf>> = None;
    tallies
        .into_iter()
        .map(|(folder, tally)| {
            if tally.is_repository {
                let common = common.get_or_insert_with(|| common_dir_of(worktree));
                let dir = worktree.join(format!("{prefix}{folder}"));
                let kind = match repository_kind(common.as_deref(), &dir) {
                    UntrackedDirectoryKind::LinkedWorktree => UntrackedFolder::LinkedWorktree,
                    UntrackedDirectoryKind::Repository | UntrackedDirectoryKind::Folder => {
                        UntrackedFolder::NestedRepository
                    }
                };
                return (folder, kind);
            }
            let fits = tally.files.len() <= *budget;
            let (fingerprint, additions) = if fits {
                *budget -= tally.files.len();
                (
                    Some(fingerprint_files(worktree, &tally.files)),
                    folder_lines(worktree, &tally.files),
                )
            } else {
                (None, 0)
            };
            // A repository inside is either one of its own or a worktree of
            // this repository, read from its `.git` file alone.
            let mut nested_repositories = 0;
            let mut linked_worktrees = 0;
            if !tally.repositories.is_empty() {
                use std::os::unix::ffi::OsStrExt;
                let common = common.get_or_insert_with(|| common_dir_of(worktree));
                for record in &tally.repositories {
                    let rel =
                        std::ffi::OsStr::from_bytes(record.strip_suffix(b"/").unwrap_or(record));
                    match repository_kind(common.as_deref(), &worktree.join(rel)) {
                        UntrackedDirectoryKind::LinkedWorktree => linked_worktrees += 1,
                        UntrackedDirectoryKind::Repository | UntrackedDirectoryKind::Folder => {
                            nested_repositories += 1
                        }
                    }
                }
            }
            // To git a bare repository is a folder of ordinary files, so they
            // are counted, staged and listed as files. Only a delete keeps
            // them, so the row says how many of its files sit in one.
            let bares = bare_repositories(worktree, &tally.files);
            let files_in_bare_repositories = if bares.is_empty() {
                0
            } else {
                tally
                    .files
                    .iter()
                    .filter(|record| bares.iter().any(|bare| is_inside(record, bare)))
                    .count()
            };
            let contents = crate::model::FolderContents {
                file_count: tally.files.len(),
                nested_repositories,
                linked_worktrees,
                fingerprint,
                repositories_not_staged: 0,
                worktrees_not_staged: 0,
                bare_repositories: bares
                    .iter()
                    .map(|bare| String::from_utf8_lossy(bare).into_owned())
                    .collect(),
                files_in_bare_repositories,
            };
            (folder, UntrackedFolder::Contents(contents, additions))
        })
        .collect()
}

/// The lines a folder's files hold between them, each counted by git's rules
/// (see `untracked_file_stat`); a binary file holds none.
fn folder_lines(worktree: &Path, records: &[&[u8]]) -> usize {
    use std::os::unix::ffi::OsStrExt;
    records
        .iter()
        .map(|record| {
            match untracked_file_stat(&worktree.join(std::ffi::OsStr::from_bytes(record))) {
                DiffStat::Text(additions, _) => additions,
                DiffStat::Binary => 0,
            }
        })
        .sum()
}

/// A hash over each file's path, size and modification time: a stat each,
/// never a read, so it is cheap next to the line counts. Records arrive in
/// `ls-files` order, which is sorted, so the same files hash the same way. A
/// file that vanished between the listing and the stat hashes as a marker.
fn fingerprint_files(worktree: &Path, records: &[&[u8]]) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::os::unix::ffi::OsStrExt;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for record in records {
        record.hash(&mut hasher);
        let path = worktree.join(std::ffi::OsStr::from_bytes(record));
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                meta.len().hash(&mut hasher);
                meta.modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| since.as_nanos())
                    .hash(&mut hasher);
            }
            Err(_) => u64::MAX.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Every untracked, not-ignored path git knows of in `worktree`, as the raw
/// NUL-separated records of `ls-files --others`, optionally limited to `dir`.
///
/// `ls-files --others --exclude-standard` answers exactly the question the
/// folded row asks ("what would `git status -uall` have listed here"): the same
/// ignore rules, and it does not descend into an ignored directory or into a
/// repository of its own, which it reports as one record ending in `/`.
/// `--full-name` keeps every record relative to the top of the worktree, the
/// same frame `git status` prints its paths in.
fn untracked_records(worktree: &Path, dir: Option<&str>) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.args([
        "--literal-pathspecs",
        "-C",
        worktree.to_string_lossy().as_ref(),
        "ls-files",
        "--others",
        "--exclude-standard",
        "--full-name",
        "-z",
    ]);
    if let Some(dir) = dir {
        // `--` so a folder named like an option is a name, and the literal
        // pathspecs flag above so one named like a glob matches only itself.
        command.arg("--").arg(format!("{dir}/"));
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// What is inside each untracked folder `git status` folded, keyed by the
/// folder's path (no trailing slash).
///
/// ONE `ls-files` over the whole worktree rather than one per folder: the
/// folders are disjoint (git never reports one folded folder inside another),
/// so each record belongs to at most one of them, found by walking the
/// record's own ancestors. Measured on a 30,000-file `node_modules` this is
/// about ten milliseconds, cheap enough to answer inside the listing rather
/// than behind a second request (see the measurements in the change that
/// introduced folding).
///
/// Records are matched as bytes, so a file whose name is not UTF-8 is still
/// counted even though it could never be a row of its own.
pub(super) fn count_untracked_folders(
    worktree: &Path,
    folders: &[String],
    budget: &mut usize,
) -> Result<HashMap<String, UntrackedFolder>> {
    if folders.is_empty() {
        return Ok(HashMap::new());
    }
    let index: HashMap<&[u8], usize> = folders
        .iter()
        .enumerate()
        .map(|(position, folder)| (folder.as_bytes(), position))
        .collect();
    let mut tallies: Vec<(String, Tally<'_>)> = folders
        .iter()
        .map(|folder| (folder.clone(), Tally::default()))
        .collect();
    // Which folders any record answered for. A folder with none is one that
    // was gone (or held nothing git lists) by the time this read ran: it gets
    // no answer at all rather than an empty one (see `apply_folder_counts`).
    let mut seen = vec![false; folders.len()];
    let raw = untracked_records(worktree, None)?;
    for record in raw.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        // The folder itself answering as `folder/` is a repository of its own.
        if let Some(&position) = record.strip_suffix(b"/").and_then(|e| index.get(e)) {
            tallies[position].1.is_repository = true;
            seen[position] = true;
            continue;
        }
        let owner = record
            .iter()
            .enumerate()
            .filter(|(position, byte)| **byte == b'/' && *position + 1 < record.len())
            .find_map(|(position, _)| index.get(&record[..position]).copied());
        if let Some(position) = owner {
            tallies[position].1.add(record);
            seen[position] = true;
        }
    }
    let tallies: Vec<(String, Tally<'_>)> = tallies
        .into_iter()
        .zip(seen)
        .filter_map(|(tally, seen)| seen.then_some(tally))
        .collect();
    Ok(finish_tallies(worktree, "", tallies, budget)
        .into_iter()
        .collect())
}

/// Give every folder row in `unstaged` what the count found in it, and drop
/// the row of a folder the count has no answer for.
///
/// `git status` and the `ls-files` the count runs are two reads, and a folder
/// deleted between them (a build cleaning its output, `rm -rf` in a terminal)
/// has no record in the second. That is a normal race, not a failure: the
/// folder is gone, so its row goes, and the next read agrees. It is never
/// shown as an empty folder, which the staged fold would otherwise have to
/// guess about.
pub(super) fn apply_folder_counts(
    unstaged: &mut Vec<ChangedFile>,
    mut counts: HashMap<String, UntrackedFolder>,
) {
    unstaged.retain_mut(|file| {
        if !file.is_folder() {
            return true;
        }
        match counts.remove(&file.path) {
            Some(folder) => {
                file.additions = folder.additions();
                file.kind = folder.kind();
                true
            }
            None => false,
        }
    });
}

/// Which of `dirs` HEAD does not have. An empty set when git could not be
/// asked, which folds nothing: a list with every staged file in it is the
/// safe way to be wrong.
///
/// `cat-file --batch-check` is plumbing and reads the names on stdin, so no
/// name can be taken for an option and no batch can outgrow the command line.
/// It answers one line per question in order: `<oid> <type> <size>`, or
/// `<question> missing`. A repository with no commits yet answers missing for
/// everything, which is right: nothing is in HEAD. A name holding a newline
/// cannot be asked on a line-based stdin, so it is never folded.
fn dirs_missing_from_head(worktree: &Path, dirs: &[&str]) -> HashSet<String> {
    let asked: Vec<&str> = dirs.iter().copied().filter(|d| !d.contains('\n')).collect();
    if asked.is_empty() {
        return HashSet::new();
    }
    let Ok(mut child) = Command::new("git")
        .args([
            "-C",
            worktree.to_string_lossy().as_ref(),
            "cat-file",
            "--batch-check",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };
    let Some(mut stdin) = child.stdin.take() else {
        return HashSet::new();
    };
    let mut payload = Vec::new();
    for dir in &asked {
        payload.extend_from_slice(b"HEAD:");
        payload.extend_from_slice(dir.as_bytes());
        payload.push(b'\n');
    }
    // Written from its own thread for the same reason `check-attr` is: a large
    // batch fills the output pipe while the input is still being fed.
    let writer = std::thread::spawn(move || {
        let _ = std::io::Write::write_all(&mut stdin, &payload);
    });
    let output = child.wait_with_output();
    let _ = writer.join();
    let Ok(output) = output else {
        return HashSet::new();
    };
    if !output.status.success() {
        return HashSet::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let answers: Vec<&str> = text.lines().collect();
    if answers.len() != asked.len() {
        return HashSet::new();
    }
    asked
        .iter()
        .zip(answers)
        .filter(|(_, answer)| answer.ends_with(" missing"))
        .map(|(dir, _)| (*dir).to_string())
        .collect()
}

/// Every proper ancestor directory of `path`, shallowest first.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(move |(index, _)| &path[..index])
}

/// Fold each folder staged whole into one row.
///
/// Staging a folded untracked folder turns it into one staged entry per file,
/// and git does not fold staged entries, so without this the staged half would
/// be exactly as long as the unstaged one used to be. A folder folds when HEAD
/// does not have it and every staged entry inside it is a plain addition: the
/// shallowest such ancestor of each added file is the fold. A rename or copy
/// inside keeps the folder open, because the row a rename needs (where it came
/// from) cannot be said by a folder.
///
/// The folder must also have been staged WHOLE: an untracked file inside it
/// (a file, or a folded folder holding files, in `unstaged`) means only part of
/// it is in the index, and a folder row would claim the rest. A repository or a
/// worktree left untracked inside it does NOT: staging a folder leaves those
/// out on purpose, so a folder staged whole beside a nested clone is still
/// whole, and its row counts what it left out instead. A tracked change inside it
/// (a staged file edited since) does not open it, because that file is in the
/// index too; the edit is its own unstaged row, and opening the folder over it
/// would put every file of a staged `node_modules` back on screen the first
/// time anything touched one.
pub(super) fn fold_added_directories(
    worktree: &Path,
    staged: Vec<ChangedFile>,
    unstaged: &[ChangedFile],
) -> Vec<ChangedFile> {
    let mut candidates: Vec<&str> = staged
        .iter()
        .filter(|file| file.status == "A")
        .flat_map(|file| ancestors(&file.path))
        .collect();
    if candidates.is_empty() {
        return staged;
    }
    candidates.sort_unstable();
    candidates.dedup();
    let missing = dirs_missing_from_head(worktree, &candidates);
    if missing.is_empty() {
        return staged;
    }

    // Folders that are not whole: every folder holding something other than a
    // plain addition, and every folder with something untracked inside it
    // (staged in part).
    let mut not_whole: HashSet<&str> = HashSet::new();
    for file in staged.iter().filter(|file| file.status != "A") {
        not_whole.extend(ancestors(&file.path));
    }
    // An intent-to-add file (`git add -N`, status ` A`) is not staged content
    // either: its index entry is a placeholder. A repository left untracked
    // (on its own row, or all a folded folder holds) is what a folder stage
    // leaves out, so it is set aside to be counted rather than breaking it.
    // Only a folder KNOWN to hold repositories and nothing else counts as
    // that: one counted empty (no files and no repositories, as when it
    // emptied between two reads) is not known to hold anything, and breaks
    // the folder around it like any untracked entry.
    let mut left_out: Vec<(&str, usize, usize)> = Vec::new();
    for file in unstaged
        .iter()
        .filter(|file| matches!(file.status.as_str(), "?" | "A"))
    {
        if file.status == "?" {
            let repositories = match &file.kind {
                ChangedFileKind::NestedRepository => Some((1, 0)),
                ChangedFileKind::LinkedWorktree => Some((0, 1)),
                ChangedFileKind::Directory(contents)
                    if contents.file_count == 0
                        && contents.nested_repositories + contents.linked_worktrees > 0 =>
                {
                    Some((contents.nested_repositories, contents.linked_worktrees))
                }
                ChangedFileKind::Directory(_) | ChangedFileKind::File => None,
            };
            if let Some((repositories, worktrees)) = repositories {
                left_out.push((file.path.as_str(), repositories, worktrees));
                continue;
            }
        }
        not_whole.insert(file.path.as_str());
        not_whole.extend(ancestors(&file.path));
    }
    // The fold root of each added file: its shallowest ancestor that HEAD
    // lacks AND that is whole. Trying each ancestor in turn, rather than only
    // the shallowest one HEAD lacks, is what lets a subfolder staged whole fold
    // under a folder staged in part. Roots never nest: a whole folder's
    // subfolders are whole too, so every file under it stops at it first.
    let root_of: Vec<Option<String>> = staged
        .iter()
        .map(|file| {
            if file.status != "A" {
                return None;
            }
            ancestors(&file.path)
                .find(|ancestor| missing.contains(*ancestor) && !not_whole.contains(ancestor))
                .map(str::to_string)
        })
        .collect();
    let mut roots: HashMap<String, usize> = HashMap::new();
    for root in root_of.iter().flatten() {
        *roots.entry(root.clone()).or_default() += 1;
    }
    if roots.is_empty() {
        return staged;
    }

    // A staged link to a repository (a gitlink, mode 160000) is not a file:
    // it is counted apart, as the untracked side counts the repositories it
    // does not enter. The folded roots are asked once, from the index.
    let root_names: Vec<&str> = roots.keys().map(String::as_str).collect();
    let links = staged_links(worktree, &root_names);
    let mut contents_of: HashMap<String, crate::model::FolderContents> = roots
        .iter()
        .map(|(root, count)| {
            (
                root.clone(),
                crate::model::FolderContents {
                    file_count: *count,
                    ..Default::default()
                },
            )
        })
        .collect();
    for (path, repositories, worktrees) in left_out {
        let Some(root) = ancestors(path).find(|ancestor| contents_of.contains_key(*ancestor))
        else {
            continue;
        };
        let contents = contents_of.get_mut(root).expect("found just above");
        contents.repositories_not_staged += repositories;
        contents.worktrees_not_staged += worktrees;
    }
    for (path, kind) in &links {
        let Some(root) = ancestors(path).find(|ancestor| contents_of.contains_key(*ancestor))
        else {
            continue;
        };
        let contents = contents_of.get_mut(root).expect("found just above");
        contents.file_count = contents.file_count.saturating_sub(1);
        match kind {
            UntrackedDirectoryKind::LinkedWorktree => contents.linked_worktrees += 1,
            UntrackedDirectoryKind::Repository | UntrackedDirectoryKind::Folder => {
                contents.nested_repositories += 1
            }
        }
    }

    let mut folded = Vec::with_capacity(staged.len());
    let mut emitted: HashSet<String> = HashSet::new();
    for (file, root) in staged.into_iter().zip(root_of) {
        match root.filter(|root| roots.contains_key(root)) {
            Some(root) => {
                if !emitted.contains(&root) {
                    let contents = contents_of.remove(&root).unwrap_or_default();
                    emitted.insert(root.clone());
                    folded.push(folder_row_with(root, "A", contents));
                }
            }
            None => folded.push(file),
        }
    }
    folded
}

/// The staged links to repositories (index mode 160000) under `dirs`, each
/// told apart into a repository of its own or a worktree of this repository
/// by reading files. One `ls-files --stage` over the folded folders; an answer
/// git could not give is no links at all.
fn staged_links(worktree: &Path, dirs: &[&str]) -> HashMap<String, UntrackedDirectoryKind> {
    let mut links = HashMap::new();
    if dirs.is_empty() {
        return links;
    }
    let Ok(output) = Command::new("git")
        .args([
            "--literal-pathspecs",
            "-C",
            worktree.to_string_lossy().as_ref(),
            "ls-files",
            "--stage",
            "-z",
            "--",
        ])
        .args(dirs)
        .output()
    else {
        return links;
    };
    if !output.status.success() {
        return links;
    }
    let mut common: Option<Option<PathBuf>> = None;
    for record in output.stdout.split(|byte| *byte == 0) {
        if !record.starts_with(b"160000 ") {
            continue;
        }
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let Ok(path) = std::str::from_utf8(&record[tab + 1..]) else {
            continue;
        };
        let common = common.get_or_insert_with(|| common_dir_of(worktree));
        let kind = repository_kind(common.as_deref(), &worktree.join(path));
        links.insert(path.to_string(), kind);
    }
    links
}

/// A folded folder row with what it holds.
fn folder_row_with(
    path: String,
    status: &str,
    contents: crate::model::FolderContents,
) -> ChangedFile {
    ChangedFile {
        status: status.to_string(),
        path,
        additions: 0,
        deletions: 0,
        binary: false,
        diff_excluded: false,
        renamed_from: None,
        kind: ChangedFileKind::Directory(contents),
    }
}

/// What an untracked directory is. The listing, the delete and the engine's
/// message all decide it the same way, from git, because deciding it from the
/// filesystem gets it wrong: an empty `.git` directory or a `.git` file that
/// points nowhere looks like a repository and is an ordinary folder to git,
/// and treating such a folder as a repository deleted its ignored files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UntrackedDirectoryKind {
    /// An ordinary folder: git lists the files inside it.
    Folder,
    /// A repository of its own: git reports the directory itself (`dir/`) and
    /// does not look inside.
    Repository,
    /// A repository git reports as its own that is really a linked worktree of
    /// this same repository.
    LinkedWorktree,
}

/// The live answer for one untracked directory: asks `ls-files --others` about
/// it, the very question the listing's batched call answers for every folded
/// folder, and then tells a linked worktree apart by reading files only.
pub fn untracked_directory_kind(worktree: &Path, rel: &str) -> Result<UntrackedDirectoryKind> {
    let raw = untracked_records(worktree, Some(rel.trim_end_matches('/')))?;
    let itself = format!("{}/", rel.trim_end_matches('/'));
    let reported_as_repository = raw
        .split(|byte| *byte == 0)
        .any(|record| record == itself.as_bytes());
    // A bare repository is never reported as one: to git it is a folder of
    // files, and it is one here too (a folder delete keeps it, see
    // `FolderInside::deletable_files`).
    if !reported_as_repository {
        return Ok(UntrackedDirectoryKind::Folder);
    }
    Ok(repository_kind(
        common_dir_of(worktree).as_deref(),
        &worktree.join(rel),
    ))
}

/// What an untracked folder holds, from the records it is counted from.
#[derive(Debug, Default)]
pub(crate) struct FolderInside {
    /// Its untracked, not-ignored files, as worktree-relative paths in git's
    /// own bytes, the files inside bare repositories included: to git, and so
    /// to a stage, those are ordinary files.
    pub(crate) files: Vec<Vec<u8>>,
    /// The bare repositories inside it (or the folder itself, when it is
    /// one), found by git's own rule for a git directory. A delete keeps them.
    pub(crate) bare: Vec<Vec<u8>>,
    /// The repositories inside it (not the folder itself), each with what it
    /// is, as worktree-relative paths in git's own bytes with no trailing
    /// slash. Bytes, because a name need not be UTF-8 and one that is not
    /// must still be left out of a stage rather than skipped.
    pub(crate) repositories: Vec<(Vec<u8>, UntrackedDirectoryKind)>,
    /// True when at least one untracked, not-ignored file is inside it, bare
    /// repositories' files included.
    pub(crate) has_file: bool,
}

impl FolderInside {
    /// How many repositories of their own, and how many worktrees of this
    /// repository, are inside.
    pub(crate) fn counts(&self) -> (usize, usize) {
        let worktrees = self
            .repositories
            .iter()
            .filter(|(_, kind)| *kind == UntrackedDirectoryKind::LinkedWorktree)
            .count();
        (self.repositories.len() - worktrees, worktrees)
    }

    /// The files a folder delete removes: every counted file outside the
    /// bare repositories, which it keeps.
    pub(crate) fn deletable_files(&self) -> impl Iterator<Item = &[u8]> {
        self.files
            .iter()
            .map(Vec::as_slice)
            .filter(|record| !self.bare.iter().any(|bare| is_inside(record, bare)))
    }
}

/// What is inside `dir`: found live from the same `ls-files --others` records
/// a folder is counted from, with each repository told apart by reading files
/// only.
pub(crate) fn folder_inside(worktree: &Path, dir: &str) -> Result<FolderInside> {
    use std::os::unix::ffi::OsStrExt;
    let dir = dir.trim_end_matches('/');
    let raw = untracked_records(worktree, Some(dir))?;
    let itself = format!("{dir}/");
    let mut common: Option<Option<PathBuf>> = None;
    let mut inside = FolderInside::default();
    let mut files: Vec<&[u8]> = Vec::new();
    for record in raw.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        if !record.ends_with(b"/") {
            files.push(record);
            continue;
        }
        if record == itself.as_bytes() {
            continue;
        }
        let rel = &record[..record.len() - 1];
        let common = common.get_or_insert_with(|| common_dir_of(worktree));
        let kind = repository_kind(
            common.as_deref(),
            &worktree.join(std::ffi::OsStr::from_bytes(rel)),
        );
        inside.repositories.push((rel.to_vec(), kind));
    }
    inside.bare = bare_repositories(worktree, &files);
    inside.files = files.into_iter().map(<[u8]>::to_vec).collect();
    inside.has_file = !inside.files.is_empty();
    Ok(inside)
}

/// A whole-folder action that leaves the repositories inside the folder alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderAction {
    Stage,
    Delete,
}

/// The sentence for a folder `action` would do nothing to, because all it
/// holds are `repositories` of their own and `worktrees` of this repository
/// (which the action leaves alone), besides files the repository ignores.
/// Worded by what is actually there: a worktree is not a repository of its
/// own, and a mix names both. One builder, so the refusal a surface raises up
/// front and the one the core answers with cannot drift apart.
pub fn nothing_in_folder(
    path: &str,
    repositories: usize,
    worktrees: usize,
    action: FolderAction,
) -> String {
    let holds = match (repositories > 0, worktrees > 0) {
        (true, false) => "only repositories of their own",
        (false, true) => "only worktrees of this repository",
        (true, true) => "only repositories of their own and worktrees of this repository",
        (false, false) => {
            return match action {
                FolderAction::Stage => {
                    format!("There is nothing in \"{path}/\" to stage: git ignores all it holds.")
                }
                FolderAction::Delete => format!(
                    "There is nothing in \"{path}/\" that a delete would remove: it holds only \
                     files the repository ignores, which a delete keeps."
                ),
            };
        }
    };
    let (opening, verb, worktree_verb) = match action {
        FolderAction::Stage => (
            format!(
                "There is nothing in \"{path}/\" to stage: it holds {holds}, which staging a \
                 folder leaves out."
            ),
            "Stage",
            "Manage",
        ),
        FolderAction::Delete => (
            format!(
                "There is nothing in \"{path}/\" that a delete would remove: it holds {holds}, \
                 which a delete keeps, besides any files the repository ignores."
            ),
            "Delete",
            "Remove",
        ),
    };
    let mut sentence = opening;
    if repositories > 0 {
        sentence.push_str(&format!(
            " {verb} a nested repository from its own row after expanding the folder."
        ));
    }
    if worktrees > 0 {
        sentence.push_str(&format!(
            " {worktree_verb} a worktree of this repository from the worktree manager."
        ));
    }
    sentence
}

/// git's own test for a git directory (`is_git_directory` in git's `setup.c`),
/// by reading files only: a `HEAD` that validates as a ref (a symlink into
/// `refs/`, or `ref: refs/...`) or an object id, and `objects/` and `refs/`
/// directories, looked for under the directory a `commondir` file names when
/// there is one. This is what makes a bare repository a repository: git lists
/// its insides as ordinary untracked files and `git clean` would delete them.
pub(crate) fn is_git_directory(dir: &Path) -> bool {
    let common = match fs::read_to_string(dir.join("commondir")) {
        Ok(text) => {
            let named = Path::new(text.trim_end_matches(['\n', '\r']));
            if named.is_absolute() {
                named.to_path_buf()
            } else {
                dir.join(named)
            }
        }
        Err(_) => dir.to_path_buf(),
    };
    common.join("objects").is_dir() && common.join("refs").is_dir() && valid_head(&dir.join("HEAD"))
}

/// git's `validate_headref`: a symlink into `refs/`, a `ref:` line naming
/// something under `refs/`, or an object id (a detached HEAD).
fn valid_head(head: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(head) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return fs::read_link(head).is_ok_and(|target| target.starts_with("refs/"));
    }
    let Ok(bytes) = fs::read(head) else {
        return false;
    };
    if let Some(rest) = bytes.strip_prefix(b"ref:") {
        let rest = rest.trim_ascii_start();
        return rest.starts_with(b"refs/");
    }
    let id = bytes.trim_ascii_end();
    matches!(id.len(), 40 | 64) && id.iter().all(u8::is_ascii_hexdigit)
}

/// The bare repositories among `records` (worktree-relative untracked file
/// records), as worktree-relative directory paths in git's own bytes with no
/// trailing slash, none inside another. A bare repository is found from the
/// `HEAD` record git lists inside it, and confirmed with [`is_git_directory`],
/// so a folder pays one look per file actually named `HEAD`.
pub(crate) fn bare_repositories(worktree: &Path, records: &[&[u8]]) -> Vec<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let mut candidates: Vec<&[u8]> = records
        .iter()
        .filter_map(|record| record.strip_suffix(b"/HEAD"))
        .collect();
    candidates.sort_by_key(|candidate| candidate.len());
    candidates.dedup();
    let mut found: Vec<Vec<u8>> = Vec::new();
    for candidate in candidates {
        if found.iter().any(|bare| is_inside(candidate, bare)) {
            continue;
        }
        if is_git_directory(&worktree.join(std::ffi::OsStr::from_bytes(candidate))) {
            found.push(candidate.to_vec());
        }
    }
    found
}

/// The nearest proper ancestor of the worktree-relative `path` that is a git
/// directory by [`is_git_directory`] (a bare repository, or some repository's
/// `.git`), or `None`. A path under one belongs to that repository and is never
/// a change of this one, whatever a folder row above it would answer. Files
/// only, a few `stat`s per ancestor, with answers shared through `seen`.
pub(crate) fn git_directory_above<'p>(
    worktree: &Path,
    path: &'p str,
    seen: &mut HashMap<String, bool>,
) -> Option<&'p str> {
    ancestors(path).find(|ancestor| {
        *seen
            .entry((*ancestor).to_string())
            .or_insert_with(|| is_git_directory(&worktree.join(ancestor)))
    })
}

/// True when `path` is strictly inside the directory `dir`.
pub(crate) fn is_inside(path: &[u8], dir: &[u8]) -> bool {
    path.len() > dir.len() && path.starts_with(dir) && path[dir.len()] == b'/'
}

/// A directory git reported as a repository of its own: a linked worktree of
/// this repository when its `.git` is a FILE whose `gitdir:` resolves into
/// `common/worktrees/`, else a repository of its own. Reads files only, so the
/// listing pays no process per folder. Anything unreadable answers
/// "repository", the kind a delete already treats most carefully.
fn repository_kind(common: Option<&Path>, dir: &Path) -> UntrackedDirectoryKind {
    let linked = common.is_some_and(|common| {
        gitfile_target(dir).is_some_and(|gitdir| gitdir.starts_with(common.join("worktrees")))
    });
    if linked {
        UntrackedDirectoryKind::LinkedWorktree
    } else {
        UntrackedDirectoryKind::Repository
    }
}

/// True when `dir` holds a `.git` file pointing into this repository's
/// `worktrees/`, read from files alone. For a caller with no listing in hand.
pub(crate) fn is_linked_worktree_dir(worktree: &Path, dir: &Path) -> bool {
    gitfile_target(dir).is_some()
        && repository_kind(common_dir_of(worktree).as_deref(), dir)
            == UntrackedDirectoryKind::LinkedWorktree
}

/// Where a `.git` FILE in `dir` points, resolved and canonicalized. `None`
/// for a `.git` directory, a missing one, or a target that does not exist.
fn gitfile_target(dir: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dir.join(".git")).ok()?;
    let target = text
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    let target = Path::new(target);
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        dir.join(target)
    };
    target.canonicalize().ok()
}

/// The git directory every worktree of `worktree`'s repository shares, found
/// by reading files only: `.git` is either that directory itself (the main
/// worktree) or a file pointing at this worktree's administration directory,
/// whose `commondir` file names the shared one.
pub(crate) fn common_dir_of(worktree: &Path) -> Option<PathBuf> {
    let gitdir = match gitfile_target(worktree) {
        Some(gitdir) => gitdir,
        None => worktree.join(".git").canonicalize().ok()?,
    };
    let common = match fs::read_to_string(gitdir.join("commondir")) {
        Ok(text) => {
            let named = Path::new(text.trim());
            if named.is_absolute() {
                named.to_path_buf()
            } else {
                gitdir.join(named)
            }
        }
        Err(_) => gitdir,
    };
    common.canonicalize().ok()
}

/// Which of `paths` are real changes on `side` of this listing (`files` is
/// that side's listing).
///
/// A path that is a row of its own answers as it is. A path inside a folded
/// folder is asked of git, in one call for the whole batch, because the folder
/// row alone would also answer for an ignored file, a file that does not
/// exist and a file inside a repository of its own, none of which is a change:
/// on the unstaged side `ls-files --others --exclude-standard` must list the
/// path (or, for a folder, something inside it, or, for a nested repository,
/// the repository itself), and on the staged side `ls-files --cached` must.
/// Anything else is left out, and the caller refuses it.
pub fn rows_answering(
    worktree: &Path,
    files: &[ChangedFile],
    side: ChangesSide,
    paths: &[String],
) -> Result<HashSet<String>> {
    let mut answered = HashSet::new();
    let mut to_confirm: Vec<&str> = Vec::new();
    for path in paths {
        match crate::model::listing_row_for(files, path) {
            Some(row) if row.path == *path => {
                answered.insert(path.clone());
            }
            Some(_) => to_confirm.push(path),
            None => {}
        }
    }
    if to_confirm.is_empty() {
        return Ok(answered);
    }
    let mut command = Command::new("git");
    command.args([
        "--literal-pathspecs",
        "-C",
        worktree.to_string_lossy().as_ref(),
        "ls-files",
        "--full-name",
        "-z",
    ]);
    match side {
        ChangesSide::Unstaged => command.args(["--others", "--exclude-standard"]),
        ChangesSide::Staged => command.arg("--cached"),
    };
    command.arg("--").args(&to_confirm);
    let output = command.output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let wanted: HashSet<&str> = to_confirm.into_iter().collect();
    for record in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|r| !r.is_empty())
    {
        let Ok(record) = std::str::from_utf8(record) else {
            continue;
        };
        // A nested repository answers as `path/`; anything else answers for
        // itself and for every asked folder above it.
        let record = record.trim_end_matches('/');
        if wanted.contains(record) {
            answered.insert(record.to_string());
        }
        for ancestor in ancestors(record) {
            if wanted.contains(ancestor) {
                answered.insert(ancestor.to_string());
            }
        }
    }
    Ok(answered)
}

/// The contents of one folded folder, one level deep: every file directly
/// inside it as an ordinary row, every folder inside it folded again with its
/// own count, and every repository inside it as a nested repository row.
///
/// `side` says which folder is meant: an untracked one lists what the working
/// tree holds (by git's ignore rules), a staged one what the index holds. The
/// paths are full worktree-relative paths, so a surface can stage, unstage or
/// discard a child exactly as it would a top-level row.
///
/// Runs git and reads files, so it belongs on a worker like every other read of
/// the worktree.
pub fn changed_dir_children(
    worktree: &Path,
    dir: &str,
    side: ChangesSide,
) -> Result<Vec<ChangedFile>> {
    let dir = dir.trim_end_matches('/');
    if dir.is_empty() {
        return Err(anyhow!("no folder was named to list"));
    }
    let mut rows = match side {
        ChangesSide::Unstaged => untracked_children(worktree, dir)?,
        ChangesSide::Staged => staged_children(worktree, dir)?,
    };
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(rows)
}

/// The first path component of `rest`, and whether anything follows it.
fn split_child(rest: &[u8]) -> (&[u8], bool) {
    match rest.iter().position(|byte| *byte == b'/') {
        Some(index) => (&rest[..index], true),
        None => (rest, false),
    }
}

fn untracked_children(worktree: &Path, dir: &str) -> Result<Vec<ChangedFile>> {
    let raw = untracked_records(worktree, Some(dir))?;
    let prefix = format!("{dir}/");
    let mut files: Vec<ChangedFile> = Vec::new();
    // Sub-folder name → what is inside it, in first-seen order.
    let mut folders: Vec<(String, Tally<'_>)> = Vec::new();
    let mut folder_index: HashMap<String, usize> = HashMap::new();
    for record in raw.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        let Some(rest) = record.strip_prefix(prefix.as_bytes()) else {
            // The folder itself is a repository of its own: nothing to list.
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let (child, deeper) = split_child(rest);
        // A child whose own name is not UTF-8 cannot be a row: skipped, the
        // way the status parser skips such a path.
        let Ok(child) = std::str::from_utf8(child) else {
            logger::debug("changed_dir_children: skipping a non-UTF-8 name");
            continue;
        };
        if !deeper {
            files.push(ChangedFile {
                status: "?".to_string(),
                path: format!("{prefix}{child}"),
                additions: 0,
                deletions: 0,
                binary: false,
                diff_excluded: false,
                renamed_from: None,
                kind: ChangedFileKind::File,
            });
            continue;
        }
        let slot = *folder_index.entry(child.to_string()).or_insert_with(|| {
            folders.push((child.to_string(), Tally::default()));
            folders.len() - 1
        });
        // `name/` alone is a repository git did not look inside.
        if rest.len() == child.len() + 1 {
            folders[slot].1.is_repository = true;
        } else {
            folders[slot].1.add(record);
        }
    }

    let mut budget = UNTRACKED_STATS_MAX_FILES;
    for file in &mut files {
        if budget == 0 {
            break;
        }
        budget -= 1;
        match untracked_file_stat(&worktree.join(&file.path)) {
            DiffStat::Text(additions, deletions) => {
                file.additions = additions;
                file.deletions = deletions;
            }
            DiffStat::Binary => file.binary = true,
        }
    }

    files.extend(
        finish_tallies(worktree, &prefix, folders, &mut budget)
            .into_iter()
            .map(|(child, folder)| ChangedFile {
                status: "?".to_string(),
                path: format!("{prefix}{child}"),
                additions: folder.additions(),
                deletions: 0,
                binary: false,
                diff_excluded: false,
                renamed_from: None,
                kind: folder.kind(),
            }),
    );
    Ok(files)
}

fn staged_children(worktree: &Path, dir: &str) -> Result<Vec<ChangedFile>> {
    let wt = worktree.to_string_lossy();
    let pathspec = format!("{dir}/");
    let output = Command::new("git")
        .args([
            "--literal-pathspecs",
            "-C",
            wt.as_ref(),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=no",
            "--",
            &pathspec,
        ])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let links = staged_links(worktree, &[dir]);
    let mut files: Vec<ChangedFile> = Vec::new();
    // Sub-folder name → what is inside it, in first-seen order.
    let mut folders: Vec<(String, crate::model::FolderContents)> = Vec::new();
    let mut folder_index: HashMap<String, usize> = HashMap::new();
    let mut entries = 0_usize;
    for entry in parse_status_porcelain_z(&output.stdout) {
        if matches!(entry.index_status, ' ' | '?') {
            continue;
        }
        let Some(rest) = entry.path.strip_prefix(&pathspec) else {
            continue;
        };
        entries += 1;
        let link = links.get(&entry.path).copied();
        let (child, deeper) = split_child(rest.as_bytes());
        let child = String::from_utf8_lossy(child).into_owned();
        if deeper {
            let slot = *folder_index.entry(child.clone()).or_insert_with(|| {
                folders.push((child.clone(), crate::model::FolderContents::default()));
                folders.len() - 1
            });
            // A link to a repository is counted apart, never as a file.
            let contents = &mut folders[slot].1;
            match link {
                Some(UntrackedDirectoryKind::LinkedWorktree) => contents.linked_worktrees += 1,
                Some(UntrackedDirectoryKind::Repository | UntrackedDirectoryKind::Folder) => {
                    contents.nested_repositories += 1
                }
                None => contents.file_count += 1,
            }
        } else {
            files.push(ChangedFile {
                status: entry.index_status.to_string(),
                path: entry.path.clone(),
                additions: 0,
                deletions: 0,
                binary: false,
                diff_excluded: false,
                renamed_from: rename_source(entry.index_status, &entry.renamed_from),
                kind: match link {
                    Some(UntrackedDirectoryKind::LinkedWorktree) => ChangedFileKind::LinkedWorktree,
                    Some(_) => ChangedFileKind::NestedRepository,
                    None => ChangedFileKind::File,
                },
            });
        }
    }

    // Line counts: every entry's when they fit the read budget, so the child
    // folders carry their sums; past it, the files listed directly and none
    // for the folders, whose blobs are left unread.
    let sum_folders = entries <= UNTRACKED_STATS_MAX_FILES;
    if !files.is_empty() || (sum_folders && !folders.is_empty()) {
        let skip: Vec<String> = if sum_folders {
            Vec::new()
        } else {
            folders
                .iter()
                .map(|(child, _)| format!("{pathspec}{child}"))
                .collect()
        };
        let numstat = staged_numstat_in(worktree, dir, &skip);
        let attribute_unset = paths_excluded_from_diffs(wt.as_ref(), &countless_paths(&[&numstat]));
        let excluded = diff_excluded_rows(worktree, &attribute_unset, &numstat, ContentSide::Index);
        for file in &mut files {
            if let Some(stat) = numstat.get(&file.path) {
                apply_stat(file, stat, &excluded);
            }
        }
        if sum_folders {
            let mut sums: HashMap<String, (usize, usize)> = HashMap::new();
            for (path, stat) in &numstat {
                let DiffStat::Text(additions, deletions) = stat else {
                    continue;
                };
                let Some(rest) = path.strip_prefix(&pathspec) else {
                    continue;
                };
                let (child, deeper) = split_child(rest.as_bytes());
                if deeper {
                    let sum = sums
                        .entry(String::from_utf8_lossy(child).into_owned())
                        .or_default();
                    sum.0 += additions;
                    sum.1 += deletions;
                }
            }
            let rows: Vec<ChangedFile> = folders
                .into_iter()
                .map(|(child, contents)| {
                    let (additions, deletions) = sums.get(&child).copied().unwrap_or_default();
                    let mut row = folder_row_with(format!("{pathspec}{child}"), "A", contents);
                    row.additions = additions;
                    row.deletions = deletions;
                    row
                })
                .collect();
            files.extend(rows);
            return Ok(files);
        }
    }

    files.extend(
        folders
            .into_iter()
            .map(|(child, contents)| folder_row_with(format!("{pathspec}{child}"), "A", contents)),
    );
    Ok(files)
}

/// Staged line counts under `dir`, leaving `skip` unread. Each spec carries its
/// own `literal` magic, so a name like a glob is still only itself; past a few
/// hundred folders to skip, nothing is skipped, which costs time only.
fn staged_numstat_in(worktree: &Path, dir: &str, skip: &[String]) -> HashMap<String, DiffStat> {
    const MAX_SKIPPED_FOLDERS: usize = 256;
    let mut command = Command::new("git");
    command
        .args([
            "-C",
            worktree.to_string_lossy().as_ref(),
            "diff",
            "--cached",
            "--numstat",
            "-z",
            "--",
        ])
        .arg(format!(":(literal){dir}/"));
    if skip.len() <= MAX_SKIPPED_FOLDERS {
        command.args(skip.iter().map(|path| format!(":(exclude,literal){path}")));
    }
    command
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_numstat(&out.stdout))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::test_support;
    use super::*;

    /// A repository with one committed file, so HEAD exists and `src/` is a
    /// directory HEAD has.
    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let git = git_in(tmp.path());
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "t@t"]);
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/lib.rs"), "fn main() {}\n").unwrap();
        git(&["add", "src/lib.rs"]);
        git(&["commit", "-q", "-m", "init"]);
        drop(git);
        tmp
    }

    fn git_in(dir: &Path) -> impl Fn(&[&str]) + '_ {
        move |args: &[&str]| {
            let out = test_support::git_command()
                .args(["-C", dir.to_string_lossy().as_ref()])
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// (path, status, kind) for every row, sorted by path.
    /// The fingerprint is left out: its value is a hash, pinned by its own
    /// tests below, and every other test compares shapes.
    fn shape(files: &[ChangedFile]) -> Vec<(String, String, ChangedFileKind)> {
        let mut rows: Vec<_> = files
            .iter()
            .map(|f| {
                let mut kind = f.kind.clone();
                if let ChangedFileKind::Directory(contents) = &mut kind {
                    contents.fingerprint = None;
                }
                (f.path.clone(), f.status.clone(), kind)
            })
            .collect();
        rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        rows
    }

    fn folder(count: usize) -> ChangedFileKind {
        ChangedFileKind::directory(count)
    }

    fn folder_with_nested(files: usize, nested: usize) -> ChangedFileKind {
        ChangedFileKind::Directory(crate::model::FolderContents {
            file_count: files,
            nested_repositories: nested,
            ..Default::default()
        })
    }

    fn fingerprint_of(files: &[ChangedFile], path: &str) -> Option<u64> {
        files
            .iter()
            .find(|f| f.path == path)
            .and_then(|f| f.folder_contents())
            .expect("a folder row")
            .fingerprint
    }

    fn file() -> ChangedFileKind {
        ChangedFileKind::File
    }

    #[test]
    fn a_wholly_untracked_folder_is_one_row_counting_its_files() {
        let repo = repo();
        let root = repo.path();
        for index in 0..120 {
            write(
                root,
                &format!("node_modules/pkg{}/f{index}.js", index / 10),
                "x\n",
            );
        }
        write(root, "notes.md", "hello\n");

        let (staged, unstaged) = changed_files(root).unwrap();

        assert!(staged.is_empty());
        assert_eq!(
            shape(&unstaged),
            vec![
                ("node_modules".to_string(), "?".to_string(), folder(120)),
                ("notes.md".to_string(), "?".to_string(), file()),
            ]
        );
        assert_eq!(crate::model::total_file_count(&unstaged), 121);
    }

    #[test]
    fn ignored_files_inside_an_untracked_folder_are_not_counted() {
        let repo = repo();
        let root = repo.path();
        write(root, ".gitignore", "*.log\n");
        write(root, "build/out.js", "x\n");
        write(root, "build/debug.log", "noise\n");
        write(root, "build/deep/more.log", "noise\n");

        let (_, unstaged) = changed_files(root).unwrap();

        let build = unstaged
            .iter()
            .find(|f| f.path == "build")
            .expect("build row");
        assert_eq!(shape(std::slice::from_ref(build))[0].2, folder(1));
    }

    #[test]
    fn a_new_file_in_a_tracked_folder_stays_its_own_row() {
        let repo = repo();
        let root = repo.path();
        write(root, "src/new.rs", "fn new() {}\n");

        let (_, unstaged) = changed_files(root).unwrap();

        assert_eq!(
            shape(&unstaged),
            vec![("src/new.rs".to_string(), "?".to_string(), file())]
        );
        assert_eq!(unstaged[0].additions, 1, "a file row keeps its line count");
    }

    #[test]
    fn an_untracked_nested_repository_is_its_own_kind_of_row() {
        let repo = repo();
        let root = repo.path();
        let nested = root.join("vendor/lib");
        fs::create_dir_all(&nested).unwrap();
        let git = git_in(&nested);
        git(&["init", "-q"]);
        write(&nested, "a.txt", "a\n");

        let (_, unstaged) = changed_files(root).unwrap();

        assert_eq!(
            shape(&unstaged),
            vec![(
                "vendor".to_string(),
                "?".to_string(),
                // git does not look inside the nested repository: it is counted
                // apart from the files, of which there are none here.
                folder_with_nested(0, 1)
            )]
        );

        let children = changed_dir_children(root, "vendor", ChangesSide::Unstaged).unwrap();
        assert_eq!(
            shape(&children),
            vec![(
                "vendor/lib".to_string(),
                "?".to_string(),
                ChangedFileKind::NestedRepository
            )]
        );
    }

    #[test]
    fn a_folder_counts_its_files_and_its_nested_repositories_apart() {
        let repo = repo();
        let root = repo.path();
        write(root, "vendor/x.js", "x\n");
        write(root, "vendor/deep/y.js", "y\n");
        for nested in ["vendor/lib", "vendor/deep/other"] {
            fs::create_dir_all(root.join(nested)).unwrap();
            git_in(&root.join(nested))(&["init", "-q"]);
            write(&root.join(nested), "inside.txt", "i\n");
        }

        let (_, unstaged) = changed_files(root).unwrap();
        assert_eq!(
            shape(&unstaged),
            vec![(
                "vendor".to_string(),
                "?".to_string(),
                folder_with_nested(2, 2)
            )]
        );
        assert_eq!(crate::model::total_file_count(&unstaged), 2, "files only");

        let children = changed_dir_children(root, "vendor", ChangesSide::Unstaged).unwrap();
        assert_eq!(
            shape(&children),
            vec![
                (
                    "vendor/deep".to_string(),
                    "?".to_string(),
                    folder_with_nested(1, 1)
                ),
                (
                    "vendor/lib".to_string(),
                    "?".to_string(),
                    ChangedFileKind::NestedRepository
                ),
                ("vendor/x.js".to_string(), "?".to_string(), file()),
            ]
        );
    }

    /// A folder holding a repository of its own and a worktree of this
    /// repository, next to one file.
    fn folder_with_repositories() -> tempfile::TempDir {
        let repo = repo();
        let root = repo.path();
        write(root, "vendor/x.js", "x\n");
        let nested = root.join("vendor/lib");
        fs::create_dir_all(&nested).unwrap();
        let git = git_in(&nested);
        git(&["init", "-q"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "user.email", "t@t"]);
        write(&nested, "own.txt", "own\n");
        git(&["add", "own.txt"]);
        git(&["commit", "-q", "-m", "own"]);
        drop(git);
        git_in(root)(&["worktree", "add", "-q", "-b", "side", "vendor/wt"]);
        repo
    }

    /// Staging a folder leaves the repositories inside it out on purpose, so
    /// they stay untracked; that must not keep the staged folder open, or every
    /// file of a staged `vendor/` beside a nested clone comes back one row
    /// each. The staged row stays one row and says what it left out. An
    /// untracked FILE inside still opens it: that one is part of the folder.
    #[test]
    fn a_folder_staged_without_its_repositories_still_folds_and_says_so() {
        let repo = folder_with_repositories();
        let root = repo.path();
        write(root, "vendor/y.js", "y\n");
        let only = root.join("vendor/sub/lib");
        fs::create_dir_all(&only).unwrap();
        git_in(&only)(&["init", "-q"]);
        write(&only, "own.txt", "own\n");

        stage_with_report(root, &["vendor".to_string()]).unwrap();
        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(staged.len(), 1, "{staged:?}");
        let row = &staged[0];
        assert_eq!(row.path, "vendor");
        let contents = row.folder_contents().expect("a folder row");
        assert_eq!(contents.file_count, 2);
        assert_eq!(contents.repositories_not_staged, 2);
        assert_eq!(contents.worktrees_not_staged, 1);
        assert_eq!(
            crate::model::folder_count_label(row).as_deref(),
            Some(
                "2 files \u{b7} 2 nested repositories and 1 worktree of this repository \
                 not staged"
            )
        );

        write(root, "vendor/z.js", "z\n");
        let (staged, _) = changed_files(root).unwrap();
        assert!(
            staged.iter().all(|f| f.path != "vendor"),
            "an untracked file inside opens it: {staged:?}"
        );
    }

    /// An untracked folder row that holds neither files nor repositories (its
    /// count came back empty, say because it emptied between two reads) is
    /// not known to hold only repositories, so it keeps the staged folder
    /// around it open rather than letting it claim to be whole.
    #[test]
    fn an_untracked_folder_counted_empty_keeps_the_staged_folder_open() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "a\n");
        git_in(root)(&["add", "--", "app/a.rs"]);
        let staged = vec![ChangedFile {
            status: "A".to_string(),
            path: "app/a.rs".to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: ChangedFileKind::File,
        }];
        let unstaged = vec![ChangedFile {
            status: "?".to_string(),
            path: "app/empty".to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: ChangedFileKind::directory(0),
        }];

        let folded = fold_added_directories(root, staged, &unstaged);

        assert!(
            folded.iter().all(|f| !f.is_folder()),
            "the folder is not whole: {folded:?}"
        );
    }

    /// A folder holding a file, a bare repository and, deeper, another bare
    /// repository beside an ignored file.
    fn folder_with_bare_repositories() -> tempfile::TempDir {
        let repo = repo();
        let root = repo.path();
        write(root, "d/a.txt", "a\n");
        write(root, "d/sub/b.txt", "b\n");
        write(root, ".gitignore", "*.env\n");
        write(root, "d/deep/secret.env", "s\n");
        git_in(root)(&["init", "-q", "--bare", "d/bare.git"]);
        git_in(root)(&["init", "-q", "--bare", "d/deep/b2.git"]);
        git_in(root)(&["init", "-q", "--bare", "top.git"]);
        repo
    }

    /// How many untracked, not-ignored files git lists under `dir`.
    fn untracked_count(root: &Path, dir: &str) -> usize {
        let out = test_support::git_command()
            .args(["-C", root.to_string_lossy().as_ref()])
            .args(["ls-files", "--others", "--exclude-standard", "-z", "--"])
            .arg(format!("{dir}/"))
            .output()
            .unwrap();
        out.stdout
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .count()
    }

    /// To git a bare repository is a folder of ordinary files, and staging
    /// follows git: they are counted as the folder's files. The row also says
    /// which of them sit in bare repositories, because a delete keeps those.
    #[test]
    fn a_bare_repository_counts_as_files_and_is_noted_for_the_delete() {
        let repo = folder_with_bare_repositories();
        let root = repo.path();
        let in_bare = untracked_count(root, "d/bare.git") + untracked_count(root, "d/deep/b2.git");

        let (_, unstaged) = changed_files(root).unwrap();

        let d = unstaged
            .iter()
            .find(|f| f.path == "d")
            .expect("the folder row");
        let contents = d.folder_contents().expect("a folder");
        assert_eq!(contents.file_count, 2 + in_bare, "{contents:?}");
        assert_eq!(contents.nested_repositories, 0, "{contents:?}");
        assert_eq!(
            contents.bare_repositories,
            ["d/bare.git", "d/deep/b2.git"],
            "{contents:?}"
        );
        assert_eq!(contents.files_in_bare_repositories, in_bare, "{contents:?}");
        let top = unstaged
            .iter()
            .find(|f| f.path == "top.git")
            .expect("top.git");
        let top_contents = top.folder_contents().expect("a folder of files");
        assert_eq!(top_contents.bare_repositories, ["top.git"]);
        assert_eq!(
            top_contents.files_in_bare_repositories,
            top_contents.file_count
        );
        assert_eq!(
            untracked_directory_kind(root, "top.git").unwrap(),
            UntrackedDirectoryKind::Folder
        );

        let children = changed_dir_children(root, "d", ChangesSide::Unstaged).unwrap();
        let bare = children.iter().find(|f| f.path == "d/bare.git").unwrap();
        assert!(bare.is_expandable(), "{bare:?}");
    }

    /// Deleting the folder deletes the files it counted outside bare
    /// repositories and keeps the bare repositories, and the ignored file.
    /// A folder that is nothing but a bare repository has nothing to delete.
    #[test]
    fn deleting_a_folder_keeps_the_bare_repositories_inside_it() {
        let repo = folder_with_bare_repositories();
        let root = repo.path();

        discard_confirmed(root, "d", true, Some(ConfirmedEntry::Folder)).unwrap();

        assert!(!root.join("d/a.txt").exists());
        assert!(!root.join("d/sub").exists(), "a directory left empty goes");
        assert!(root.join("d/bare.git/HEAD").exists());
        assert!(root.join("d/deep/b2.git/HEAD").exists());
        assert!(root.join("d/deep/secret.env").exists());
        let refusal =
            discard_confirmed(root, "top.git", true, Some(ConfirmedEntry::Folder)).unwrap_err();
        assert!(refusal.to_string().contains("nothing in"), "{refusal}");
        assert!(root.join("top.git/HEAD").exists());
    }

    /// Staging a folder stages a bare repository inside it as the files git
    /// sees, the way `git add` does, and one staged on its own stages whole.
    #[test]
    fn staging_a_folder_stages_its_bare_repositories_as_files() {
        let repo = folder_with_bare_repositories();
        let root = repo.path();

        let report = stage_with_report(root, &["d".to_string()]).unwrap();
        stage_with_report(root, &["top.git".to_string()]).unwrap();

        assert_eq!(report.left_out_repositories, 0);
        let index: Vec<String> = index_modes(root)
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        for path in [
            "d/a.txt",
            "d/bare.git/HEAD",
            "d/deep/b2.git/HEAD",
            "top.git/HEAD",
        ] {
            assert!(index.iter().any(|p| p == path), "{path} in {index:?}");
        }
    }

    /// A bare repository kept as a test fixture is tracked like any file: an
    /// edit inside it stages, and a new fixture folded in a folder stages
    /// whole, file by file included. Only a discard reached through a folded
    /// folder is kept out of one.
    #[test]
    fn a_tracked_bare_fixture_stages_like_any_file() {
        let repo = repo();
        let root = repo.path();
        git_in(root)(&["init", "-q", "--bare", "testdata/fixture.git"]);
        git_in(root)(&["add", "--", "testdata"]);
        git_in(root)(&["commit", "-q", "-m", "fixture"]);
        let config = root.join("testdata/fixture.git/config");
        fs::write(
            &config,
            format!("{}# edited\n", fs::read_to_string(&config).unwrap()),
        )
        .unwrap();
        git_in(root)(&["init", "-q", "--bare", "testdata/new.git"]);

        stage_with_report(root, &["testdata/fixture.git/config".to_string()]).unwrap();
        let (_, unstaged) = changed_files(root).unwrap();
        let asked = vec![
            "testdata/new.git/config".to_string(),
            "testdata/new.git/HEAD".to_string(),
        ];
        let answered = rows_answering(root, &unstaged, ChangesSide::Unstaged, &asked).unwrap();
        stage_with_report(root, &["testdata/new.git".to_string()]).unwrap();

        assert_eq!(answered.len(), 2, "{answered:?}");
        let (staged, _) = changed_files(root).unwrap();
        assert!(
            staged
                .iter()
                .any(|f| f.path == "testdata/fixture.git/config" && f.status == "M")
        );
        assert!(
            staged
                .iter()
                .any(|f| f.path == "testdata/new.git" && f.is_folder()),
            "{staged:?}"
        );
        write(root, "d/a.txt", "a\n");
        git_in(root)(&["init", "-q", "--bare", "d/b.git"]);
        assert!(discard_classify(root, "d/b.git/HEAD").is_err());
        assert!(discard_classify(root, "d/a.txt").unwrap());
    }

    /// git's own rule: a HEAD that names a ref or an object, and objects/ and
    /// refs/ directories. A folder that merely has a file called HEAD is not one.
    #[test]
    fn only_a_real_git_directory_counts_as_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("HEAD"), "not a ref\n").unwrap();
        fs::create_dir_all(dir.join("objects")).unwrap();
        fs::create_dir_all(dir.join("refs")).unwrap();
        assert!(!is_git_directory(dir));
        fs::write(dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(is_git_directory(dir));
        fs::write(dir.join("HEAD"), format!("{}\n", "a".repeat(40))).unwrap();
        assert!(is_git_directory(dir));
        fs::remove_dir(dir.join("refs")).unwrap();
        assert!(!is_git_directory(dir));
    }

    /// The dialog promised one file; if a folder has taken its name before
    /// the confirm lands, the delete refuses and cleans nothing.
    #[test]
    fn a_file_that_became_a_folder_after_the_dialog_is_not_cleaned() {
        let repo = repo();
        let root = repo.path();
        write(root, "x", "one file\n");
        assert!(discard_classify(root, "x").unwrap());
        fs::remove_file(root.join("x")).unwrap();
        write(root, "x/a.txt", "a\n");
        write(root, "x/b.txt", "b\n");
        assert!(discard_classify(root, "x").unwrap(), "the race is real");

        let refusal = discard_confirmed(root, "x", true, Some(ConfirmedEntry::File)).unwrap_err();

        assert!(
            refusal.to_string().contains("changed since you looked"),
            "{refusal}"
        );
        assert!(root.join("x/a.txt").exists() && root.join("x/b.txt").exists());
    }

    /// With nothing confirmed, which is what an older client sends, a plain
    /// file is still deleted, but a directory is refused: nothing said the
    /// user was looking at a folder.
    #[test]
    fn an_unconfirmed_discard_deletes_a_file_but_never_a_folder() {
        let repo = repo();
        let root = repo.path();
        write(root, "loose.txt", "x\n");
        write(root, "dir/a.txt", "a\n");

        discard_confirmed(root, "loose.txt", true, None).unwrap();
        let refusal = discard_confirmed(root, "dir", true, None).unwrap_err();

        assert!(!root.join("loose.txt").exists());
        assert!(root.join("dir/a.txt").exists());
        assert!(refusal.to_string().contains("\"dir/\""), "{refusal}");
    }

    /// A folder `git status` listed but that is gone by the time it is
    /// counted gets no answer from the count, rather than an invented empty
    /// one; the listing then drops its row, which the next read agrees with.
    #[test]
    fn a_folder_gone_before_it_is_counted_gets_no_answer_and_no_row() {
        let repo = repo();
        let root = repo.path();
        write(root, "here/a.txt", "a\n");
        let mut budget = 100;

        let counts =
            count_untracked_folders(root, &["here".to_string(), "gone".to_string()], &mut budget)
                .unwrap();

        assert!(counts.contains_key("here"));
        assert!(
            !counts.contains_key("gone"),
            "an unseen folder has no answer"
        );

        let row = |path: &str| ChangedFile {
            status: "?".to_string(),
            path: path.to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: ChangedFileKind::directory(0),
        };
        let mut unstaged = vec![row("here"), row("gone")];
        apply_folder_counts(&mut unstaged, counts);
        assert_eq!(
            unstaged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["here"]
        );
        assert_eq!(unstaged[0].file_count(), 1);
    }

    fn index_modes(root: &Path) -> Vec<(String, String)> {
        let out = test_support::git_command()
            .args([
                "-C",
                root.to_string_lossy().as_ref(),
                "ls-files",
                "--stage",
                "-z",
            ])
            .output()
            .unwrap();
        out.stdout
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|r| {
                let text = String::from_utf8_lossy(r);
                let (meta, path) = text.split_once('\t').unwrap();
                (
                    path.to_string(),
                    meta.split(' ').next().unwrap().to_string(),
                )
            })
            .collect()
    }

    /// Staging a folder stages exactly what its count covers, its files: the
    /// repositories inside it are left out rather than recorded as links, and
    /// the report says how many.
    #[test]
    fn staging_a_folder_leaves_its_repositories_out() {
        let repo = folder_with_repositories();
        let root = repo.path();

        let report = stage_with_report(root, &["vendor".to_string()]).unwrap();

        assert_eq!(
            (report.left_out_repositories, report.left_out_worktrees),
            (1, 1)
        );
        let modes = index_modes(root);
        assert!(modes.iter().all(|(_, mode)| mode != "160000"), "{modes:?}");
        assert!(modes.iter().any(|(path, _)| path == "vendor/x.js"));
        stage_file(root, "vendor").unwrap();
        assert!(index_modes(root).iter().all(|(_, mode)| mode != "160000"));
    }

    /// Staging a new module must not make its lines vanish from the staged
    /// recap: a staged folder within the read budget carries its files' sums.
    #[test]
    fn a_staged_folder_carries_the_lines_of_its_files() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "1\n2\n");
        write(root, "app/sub/b.rs", "1\n");
        git_in(root)(&["add", "--", "app"]);

        let (staged, _) = changed_files(root).unwrap();
        let row = staged.iter().find(|f| f.path == "app").unwrap();
        assert!(row.is_expandable());
        assert_eq!((row.additions, row.deletions), (3, 0));

        let children = changed_dir_children(root, "app", ChangesSide::Staged).unwrap();
        let sub = children.iter().find(|f| f.path == "app/sub").unwrap();
        assert_eq!(sub.additions, 1);
    }

    #[test]
    fn a_staged_folder_over_the_budget_carries_no_line_counts() {
        let repo = repo();
        let root = repo.path();
        for index in 0..=UNTRACKED_STATS_MAX_FILES {
            write(root, &format!("big/f{index}.js"), "x\n");
        }
        git_in(root)(&["add", "--", "big"]);

        let (staged, _) = changed_files(root).unwrap();
        let row = staged.iter().find(|f| f.path == "big").unwrap();
        assert_eq!(row.additions, 0);
    }

    /// A folder holding nothing but repositories has nothing a stage would
    /// add, since those are left out: the stage says so instead of claiming
    /// it staged the folder. The sentence names what it holds.
    #[test]
    fn staging_a_folder_that_holds_only_repositories_is_refused() {
        let repo = repo();
        let root = repo.path();
        let nested = root.join("only-repo/lib");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "own.txt", "own\n");
        git_in(root)(&["worktree", "add", "-q", "-b", "a", "only-wt/wt"]);
        git_in(root)(&["worktree", "add", "-q", "-b", "b", "mixed/wt"]);
        let mixed = root.join("mixed/lib");
        fs::create_dir_all(&mixed).unwrap();
        git_in(&mixed)(&["init", "-q"]);
        write(&mixed, "own.txt", "own\n");

        for (folder, holds) in [
            ("only-repo", "only repositories of their own"),
            ("only-wt", "only worktrees of this repository"),
            (
                "mixed",
                "only repositories of their own and worktrees of this repository",
            ),
        ] {
            let refusal = stage_with_report(root, &[folder.to_string()]).unwrap_err();
            let text = refusal.to_string();
            assert!(
                text.contains(&format!("nothing in \"{folder}/\" to stage")),
                "{text}"
            );
            assert!(text.contains(holds), "{text}");
            let refusal = discard_confirmed(root, folder, true, Some(ConfirmedEntry::Folder))
                .unwrap_err()
                .to_string();
            assert!(refusal.contains(holds), "{refusal}");
        }
        let (staged, _) = changed_files(root).unwrap();
        assert!(staged.is_empty(), "{staged:?}");
    }

    /// A repository whose name is not UTF-8 is still left out of a folder
    /// stage: excluded by its bytes, not skipped because it has no `String`.
    #[test]
    fn a_repository_with_a_non_utf8_name_is_still_left_out() {
        use std::os::unix::ffi::OsStrExt;
        let repo = repo();
        let root = repo.path();
        write(root, "vendor/x.js", "x\n");
        let nested = root
            .join("vendor")
            .join(std::ffi::OsStr::from_bytes(b"caf\xe9"));
        fs::create_dir_all(&nested).unwrap();
        // `git -C` would need the name as text; run inside the folder instead.
        let git = |args: &[&str]| {
            let out = test_support::git_command()
                .current_dir(&nested)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.name", "t"]);
        git(&["config", "user.email", "t@t"]);
        fs::write(nested.join("own.txt"), "own\n").unwrap();
        git(&["add", "own.txt"]);
        git(&["commit", "-q", "-m", "own"]);

        let report = stage_with_report(root, &["vendor".to_string()]).unwrap();

        assert_eq!(report.left_out_repositories, 1);
        assert!(index_modes(root).iter().all(|(_, mode)| mode != "160000"));
    }

    /// Links already staged by hand are counted apart, never as files.
    #[test]
    fn a_staged_folder_counts_links_to_repositories_apart_from_files() {
        let repo = folder_with_repositories();
        let root = repo.path();
        git_in(root)(&["add", "--", "vendor"]);

        let (staged, _) = changed_files(root).unwrap();

        let row = staged
            .iter()
            .find(|f| f.path == "vendor")
            .expect("vendor row");
        let contents = row.folder_contents().unwrap();
        assert_eq!(
            (
                contents.file_count,
                contents.nested_repositories,
                contents.linked_worktrees
            ),
            (1, 1, 1)
        );
    }

    /// A worktree of this same repository inside a folded folder is not a
    /// repository of its own and is counted apart from them.
    #[test]
    fn a_folder_counts_worktrees_of_this_repository_apart() {
        let repo = repo();
        let root = repo.path();
        write(root, "vendor/x.js", "x\n");
        let nested = root.join("vendor/lib");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "own.txt", "own\n");
        git_in(root)(&["worktree", "add", "-q", "-b", "side", "vendor/wt"]);

        let (_, unstaged) = changed_files(root).unwrap();

        let row = unstaged.iter().find(|f| f.path == "vendor").unwrap();
        let contents = row.folder_contents().unwrap();
        assert_eq!(
            (
                contents.file_count,
                contents.nested_repositories,
                contents.linked_worktrees
            ),
            (1, 1, 1)
        );
        assert_eq!(
            crate::model::folder_count_label(row).as_deref(),
            Some("1 file, 1 nested repository and 1 worktree of this repository")
        );
    }

    /// Editing a file inside a folded folder changes neither its path nor its
    /// count, so the fingerprint is what moves.
    #[test]
    fn a_folder_fingerprint_moves_when_a_file_inside_is_edited() {
        let repo = repo();
        let root = repo.path();
        write(root, "dist/a.js", "a\n");
        write(root, "dist/sub/b.js", "b\n");

        let (_, first) = changed_files(root).unwrap();
        let (_, again) = changed_files(root).unwrap();
        let before = fingerprint_of(&first, "dist");
        assert!(before.is_some(), "a small folder is fingerprinted");
        assert_eq!(
            before,
            fingerprint_of(&again, "dist"),
            "stable while nothing changes"
        );

        write(root, "dist/sub/b.js", "b\nand more\n");
        let (_, edited) = changed_files(root).unwrap();
        assert_ne!(before, fingerprint_of(&edited, "dist"));

        let children = changed_dir_children(root, "dist", ChangesSide::Unstaged).unwrap();
        assert!(fingerprint_of(&children, "dist/sub").is_some());
    }

    /// A new module is a folded folder, and the lines written into it are still
    /// lines: the row carries the sum of its files' line counts, read within
    /// the same budget that already bounds the untracked files' counts.
    #[test]
    fn a_folder_row_carries_the_lines_of_its_files() {
        let repo = repo();
        let root = repo.path();
        write(root, "module/a.rs", "1\n2\n");
        write(root, "module/sub/b.rs", "1\n");
        write(root, "module/c.rs", "1\n2\n3\n");
        fs::write(root.join("module/blob.bin"), [0u8, 1, 2, 3]).unwrap();

        let (_, unstaged) = changed_files(root).unwrap();
        let row = unstaged.iter().find(|f| f.path == "module").unwrap();
        assert_eq!((row.additions, row.deletions), (6, 0));

        let children = changed_dir_children(root, "module", ChangesSide::Unstaged).unwrap();
        let sub = children.iter().find(|f| f.path == "module/sub").unwrap();
        assert_eq!(sub.additions, 1);
    }

    /// Loose untracked files are served first and folders from what is left,
    /// so a file beside a large folder keeps the count it always had.
    #[test]
    fn a_loose_file_keeps_its_count_beside_a_folder_that_fills_the_budget() {
        let repo = repo();
        let root = repo.path();
        for index in 0..UNTRACKED_STATS_MAX_FILES {
            write(root, &format!("big/f{index}.js"), "x\n");
        }
        write(root, "a.txt", "one\ntwo\n");

        let (_, unstaged) = changed_files(root).unwrap();

        let loose = unstaged.iter().find(|f| f.path == "a.txt").unwrap();
        assert_eq!(loose.additions, 2);
        let big = unstaged.iter().find(|f| f.path == "big").unwrap();
        assert_eq!(big.additions, 0, "the folder no longer fits what is left");
    }

    #[test]
    fn a_folder_over_the_budget_carries_no_line_counts() {
        let repo = repo();
        let root = repo.path();
        for index in 0..=UNTRACKED_STATS_MAX_FILES {
            write(root, &format!("big/f{index}.js"), "x\n");
        }

        let (_, unstaged) = changed_files(root).unwrap();
        let row = unstaged.iter().find(|f| f.path == "big").unwrap();
        assert_eq!(row.additions, 0);
    }

    /// Past the stat budget a folder is followed by its count alone: the
    /// accepted gap, since statting a whole dependency folder on every read is
    /// the cost folding exists to avoid.
    #[test]
    fn a_folder_over_the_budget_carries_no_fingerprint() {
        let repo = repo();
        let root = repo.path();
        for index in 0..=UNTRACKED_STATS_MAX_FILES {
            write(root, &format!("big/f{index}.js"), "x\n");
        }
        write(root, "small/a.js", "a\n");

        let (_, unstaged) = changed_files(root).unwrap();

        assert_eq!(fingerprint_of(&unstaged, "big"), None);
        assert!(fingerprint_of(&unstaged, "small").is_some());
    }

    #[test]
    fn a_staged_folder_carries_no_fingerprint() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "a\n");
        git_in(root)(&["add", "--", "app"]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(fingerprint_of(&staged, "app"), None);
    }

    #[test]
    fn a_nested_repository_at_the_top_is_reported_as_one() {
        let repo = repo();
        let root = repo.path();
        let nested = root.join("clone");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "a.txt", "a\n");

        let (_, unstaged) = changed_files(root).unwrap();

        assert_eq!(
            shape(&unstaged),
            vec![(
                "clone".to_string(),
                "?".to_string(),
                ChangedFileKind::NestedRepository
            )]
        );
    }

    #[test]
    fn expanding_an_untracked_folder_lists_one_level_with_subfolders_folded() {
        let repo = repo();
        let root = repo.path();
        write(root, "node_modules/top.js", "one\ntwo\n");
        write(root, "node_modules/pkg/a.js", "a\n");
        write(root, "node_modules/pkg/lib/b.js", "b\n");
        write(root, "node_modules/other/c.js", "c\n");

        let children = changed_dir_children(root, "node_modules", ChangesSide::Unstaged).unwrap();

        assert_eq!(
            shape(&children),
            vec![
                ("node_modules/other".to_string(), "?".to_string(), folder(1)),
                ("node_modules/pkg".to_string(), "?".to_string(), folder(2)),
                ("node_modules/top.js".to_string(), "?".to_string(), file()),
            ]
        );
        let top = children
            .iter()
            .find(|f| f.path == "node_modules/top.js")
            .unwrap();
        assert_eq!(top.additions, 2, "a listed file carries its line count");

        let deeper = changed_dir_children(root, "node_modules/pkg", ChangesSide::Unstaged).unwrap();
        assert_eq!(
            shape(&deeper),
            vec![
                ("node_modules/pkg/a.js".to_string(), "?".to_string(), file()),
                (
                    "node_modules/pkg/lib".to_string(),
                    "?".to_string(),
                    folder(1)
                ),
            ]
        );
    }

    #[test]
    fn a_folder_staged_whole_is_one_staged_row() {
        let repo = repo();
        let root = repo.path();
        for index in 0..30 {
            write(
                root,
                &format!("node_modules/p{}/f{index}.js", index % 3),
                "x\n",
            );
        }
        write(root, "src/lib.rs", "fn main() { changed(); }\n");
        let git = git_in(root);
        git(&["add", "--", "node_modules", "src/lib.rs"]);

        let (staged, unstaged) = changed_files(root).unwrap();

        assert!(unstaged.is_empty(), "{unstaged:?}");
        assert_eq!(
            shape(&staged),
            vec![
                ("node_modules".to_string(), "A".to_string(), folder(30)),
                ("src/lib.rs".to_string(), "M".to_string(), file()),
            ]
        );

        let children = changed_dir_children(root, "node_modules", ChangesSide::Staged).unwrap();
        assert_eq!(
            shape(&children),
            vec![
                ("node_modules/p0".to_string(), "A".to_string(), folder(10)),
                ("node_modules/p1".to_string(), "A".to_string(), folder(10)),
                ("node_modules/p2".to_string(), "A".to_string(), folder(10)),
            ]
        );
        let files = changed_dir_children(root, "node_modules/p0", ChangesSide::Staged).unwrap();
        assert_eq!(files.len(), 10);
        assert!(files.iter().all(|f| f.status == "A" && f.additions == 1));
    }

    /// A folder staged whole has no line counts to show, so the staged numstat
    /// does not read its blobs: on a 30,000-file folder that read was most of
    /// the listing's time. The rows left open keep their counts.
    #[test]
    fn staged_line_counts_skip_the_folders_that_were_folded() {
        let repo = repo();
        let root = repo.path();
        write(root, "a*b/one.js", "1\n2\n");
        write(root, "a*b/two.js", "1\n");
        write(root, "axb/decoy.js", "1\n");
        write(root, "src/lib.rs", "fn main() { changed(); }\nmore\n");
        git_in(root)(&["add", "--", "."]);

        let stats = staged_numstat(&root.to_string_lossy(), &["a*b".to_string()]);

        let mut paths: Vec<_> = stats.keys().cloned().collect();
        paths.sort();
        assert_eq!(
            paths,
            vec!["axb/decoy.js".to_string(), "src/lib.rs".to_string()]
        );

        let (staged, _) = changed_files(root).unwrap();
        let lib = staged.iter().find(|f| f.path == "src/lib.rs").unwrap();
        assert_eq!((lib.additions, lib.deletions), (2, 1));
    }

    /// Staging one file of a new folder is not staging the folder: the rest is
    /// still untracked, and a folder row would claim the whole of it.
    #[test]
    fn a_folder_staged_in_part_stays_one_row_per_staged_file() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/feature/a.rs", "a\n");
        write(root, "app/feature/b.rs", "b\n");
        write(root, "app/top.rs", "t\n");
        git_in(root)(&["add", "--", "app/feature/a.rs"]);

        let (staged, unstaged) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![("app/feature/a.rs".to_string(), "A".to_string(), file())]
        );
        assert_eq!(staged[0].additions, 1, "a file row keeps its line count");
        assert_eq!(crate::model::total_file_count(&unstaged), 2);
    }

    /// Staging one whole subfolder of an untracked folder folds that subfolder,
    /// even though the folder above it is only staged in part.
    #[test]
    fn a_subfolder_staged_whole_folds_under_a_folder_staged_in_part() {
        let repo = repo();
        let root = repo.path();
        for index in 0..50 {
            write(root, &format!("node_modules/big/f{index}.js"), "x\n");
        }
        write(root, "node_modules/other/x.js", "x\n");
        git_in(root)(&["add", "--", "node_modules/big"]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![("node_modules/big".to_string(), "A".to_string(), folder(50))]
        );
    }

    /// Unstaging one file of a folder staged whole opens only the folders on
    /// that file's way down; its sibling folders stay folded.
    #[test]
    fn unstaging_one_file_opens_only_the_folders_that_held_it() {
        let repo = repo();
        let root = repo.path();
        for index in 0..12 {
            write(
                root,
                &format!("node_modules/pkg{}/f{index}.js", index / 4),
                "x\n",
            );
        }
        git_in(root)(&["add", "--", "node_modules"]);
        git_in(root)(&["reset", "-q", "HEAD", "--", "node_modules/pkg0/f0.js"]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![
                (
                    "node_modules/pkg0/f1.js".to_string(),
                    "A".to_string(),
                    file()
                ),
                (
                    "node_modules/pkg0/f2.js".to_string(),
                    "A".to_string(),
                    file()
                ),
                (
                    "node_modules/pkg0/f3.js".to_string(),
                    "A".to_string(),
                    file()
                ),
                ("node_modules/pkg1".to_string(), "A".to_string(), folder(4)),
                ("node_modules/pkg2".to_string(), "A".to_string(), folder(4)),
            ]
        );
    }

    /// A file added with intent-to-add (`git add -N`) is not staged content:
    /// the folder holding it is staged in part, as with an untracked file.
    #[test]
    fn an_intent_to_add_file_inside_a_folder_keeps_it_open() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "a\n");
        write(root, "app/b.rs", "b\n");
        git_in(root)(&["add", "--", "app/a.rs"]);
        git_in(root)(&["add", "-N", "--", "app/b.rs"]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![("app/a.rs".to_string(), "A".to_string(), file())]
        );
    }

    /// Something untracked appearing inside a folder staged whole makes it a
    /// folder staged in part, so it opens up again.
    #[test]
    fn an_untracked_file_inside_a_staged_folder_opens_it() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "a\n");
        write(root, "app/b.rs", "b\n");
        git_in(root)(&["add", "--", "app"]);
        write(root, "app/late.rs", "late\n");

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![
                ("app/a.rs".to_string(), "A".to_string(), file()),
                ("app/b.rs".to_string(), "A".to_string(), file()),
            ]
        );
    }

    /// Editing a file after staging the folder whole leaves the folder staged
    /// whole: every file in it is in the index, and the edit is its own
    /// unstaged row. Opening the folder here would put every file of a staged
    /// `node_modules` back on screen the first time anything touched one.
    #[test]
    fn an_edit_after_staging_a_folder_whole_keeps_it_one_row() {
        let repo = repo();
        let root = repo.path();
        write(root, "app/a.rs", "a\n");
        write(root, "app/b.rs", "b\n");
        git_in(root)(&["add", "--", "app"]);
        write(root, "app/a.rs", "a\nmore\n");

        let (staged, unstaged) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![("app".to_string(), "A".to_string(), folder(2))]
        );
        assert_eq!(
            shape(&unstaged),
            vec![("app/a.rs".to_string(), "M".to_string(), file())]
        );
    }

    #[test]
    fn a_new_folder_inside_a_tracked_one_folds_at_the_new_folder() {
        let repo = repo();
        let root = repo.path();
        write(root, "src/feature/a.rs", "a\n");
        write(root, "src/feature/b.rs", "b\n");
        write(root, "src/top.rs", "t\n");
        git_in(root)(&["add", "--", "src"]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![
                ("src/feature".to_string(), "A".to_string(), folder(2)),
                ("src/top.rs".to_string(), "A".to_string(), file()),
            ]
        );
    }

    #[test]
    fn a_staged_rename_into_a_new_folder_keeps_the_folder_open() {
        let repo = repo();
        let root = repo.path();
        let git = git_in(root);
        fs::create_dir_all(root.join("moved")).unwrap();
        git(&["mv", "src/lib.rs", "moved/lib.rs"]);
        write(root, "moved/extra.rs", "e\n");
        git(&["add", "--", "moved"]);

        let (staged, _) = changed_files(root).unwrap();

        let paths: Vec<_> = staged.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.contains(&"moved/lib.rs") && paths.contains(&"moved/extra.rs"),
            "a folder holding a rename is not folded, so the rename stays visible: {paths:?}"
        );
    }

    #[test]
    fn staging_in_a_repository_with_no_commits_folds_new_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let git = git_in(root);
        git(&["init", "-q", "-b", "main"]);
        write(root, "app/a.rs", "a\n");
        write(root, "app/b.rs", "b\n");
        write(root, "README", "r\n");
        git(&["add", "--", "."]);

        let (staged, _) = changed_files(root).unwrap();

        assert_eq!(
            shape(&staged),
            vec![
                ("README".to_string(), "A".to_string(), file()),
                ("app".to_string(), "A".to_string(), folder(2)),
            ]
        );
    }

    /// Before the first commit there is no HEAD to reset to, and the batch
    /// unstage must still take a folded new folder back out of the index, as
    /// the single-path unstage does.
    #[test]
    fn a_batch_unstage_works_before_the_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git_in(root)(&["init", "-q", "-b", "main"]);
        write(root, "app/a.rs", "a\n");
        write(root, "app/sub/b.rs", "b\n");
        write(root, "README", "r\n");
        stage_files(root, &["app".to_string(), "README".to_string()]).unwrap();
        let (staged, _) = changed_files(root).unwrap();
        assert!(staged.iter().any(|f| f.path == "app" && f.is_folder()));

        unstage_files(root, &["app".to_string()]).unwrap();

        let (staged, unstaged) = changed_files(root).unwrap();
        assert_eq!(
            staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["README"]
        );
        assert!(unstaged.iter().any(|f| f.path == "app" && f.is_folder()));
    }

    #[test]
    fn folder_names_that_look_like_options_or_globs_are_names() {
        let repo = repo();
        let root = repo.path();
        write(root, "-rf/a.txt", "a\n");
        write(root, "a*b/c.txt", "c\n");
        write(root, "ab/decoy.txt", "d\n");
        write(root, "aXb/decoy.txt", "d\n");

        let (_, unstaged) = changed_files(root).unwrap();
        let dash = unstaged.iter().find(|f| f.path == "-rf").expect("dash row");
        assert_eq!(shape(std::slice::from_ref(dash))[0].2, folder(1));
        let glob = unstaged.iter().find(|f| f.path == "a*b").expect("glob row");
        assert_eq!(shape(std::slice::from_ref(glob))[0].2, folder(1));

        let children = changed_dir_children(root, "a*b", ChangesSide::Unstaged).unwrap();
        assert_eq!(
            shape(&children),
            vec![("a*b/c.txt".to_string(), "?".to_string(), file())]
        );
        let children = changed_dir_children(root, "-rf", ChangesSide::Unstaged).unwrap();
        assert_eq!(
            shape(&children),
            vec![("-rf/a.txt".to_string(), "?".to_string(), file())]
        );
    }

    #[test]
    fn a_file_inside_an_untracked_folder_is_classified_untracked_for_discard() {
        let repo = repo();
        let root = repo.path();
        write(root, "node_modules/pkg/a.js", "a\n");

        assert!(discard_classify(root, "node_modules").unwrap());
        assert!(discard_classify(root, "node_modules/pkg/a.js").unwrap());
        assert!(discard_classify(root, "node_modules/pkg").unwrap());
        assert!(discard_classify(root, "elsewhere.txt").is_err());
    }

    #[test]
    fn a_file_inside_a_staged_folder_must_be_unstaged_before_discarding() {
        let repo = repo();
        let root = repo.path();
        write(root, "node_modules/pkg/a.js", "a\n");
        git_in(root)(&["add", "--", "node_modules"]);

        let refusal = discard_classify(root, "node_modules/pkg/a.js").unwrap_err();
        assert!(refusal.to_string().contains("Unstage"), "{refusal}");
    }

    #[test]
    fn staging_and_unstaging_a_folder_moves_it_whole() {
        let repo = repo();
        let root = repo.path();
        for index in 0..5 {
            write(root, &format!("dist/f{index}.js"), "x\n");
        }

        stage_file(root, "dist").unwrap();
        let (staged, unstaged) = changed_files(root).unwrap();
        assert_eq!(
            shape(&staged),
            vec![("dist".to_string(), "A".to_string(), folder(5))]
        );
        assert!(unstaged.is_empty());

        unstage_file(root, "dist").unwrap();
        let (staged, unstaged) = changed_files(root).unwrap();
        assert!(staged.is_empty());
        assert_eq!(
            shape(&unstaged),
            vec![("dist".to_string(), "?".to_string(), folder(5))]
        );

        discard_confirmed(root, "dist", true, Some(ConfirmedEntry::Folder)).unwrap();
        let (staged, unstaged) = changed_files(root).unwrap();
        assert!(staged.is_empty() && unstaged.is_empty());
        assert!(!root.join("dist").exists());
    }

    // ── Deleting a folder ──────────────────────────────────────────────────
    //
    // Discarding an untracked folder deletes exactly what its row counted:
    // the untracked files git would list. Files the repository ignores (a
    // local `.env`, say) were never counted or shown, so they stay, and so do
    // repositories of their own inside it, which git does not look into.

    #[test]
    fn discarding_an_untracked_folder_keeps_ignored_files_and_nested_repositories() {
        let repo = repo();
        let root = repo.path();
        write(root, ".gitignore", "*.env\n");
        git_in(root)(&["add", ".gitignore"]);
        git_in(root)(&["commit", "-q", "-m", "ignore"]);
        write(root, "config/app.js", "a\n");
        write(root, "config/sub/z.js", "z\n");
        write(root, "config/prod.env", "SECRET=1\n");
        let nested = root.join("config/nested");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "own.txt", "own\n");

        assert!(discard_classify(root, "config").unwrap());
        discard_confirmed(root, "config", true, Some(ConfirmedEntry::Folder)).unwrap();

        assert!(!root.join("config/app.js").exists());
        assert!(!root.join("config/sub").exists());
        assert_eq!(
            fs::read_to_string(root.join("config/prod.env")).unwrap(),
            "SECRET=1\n",
            "an ignored file is not part of the folder's changes and is kept"
        );
        assert!(nested.join(".git").exists(), "a nested repository is kept");
        assert!(nested.join("own.txt").exists());
    }

    #[test]
    fn deleting_folders_named_like_options_or_globs_touches_nothing_else() {
        let repo = repo();
        let root = repo.path();
        write(root, "-rf/a.txt", "a\n");
        write(root, "a*b/c.txt", "c\n");
        write(root, "ab/keep.txt", "keep\n");
        write(root, "axb/keep.txt", "keep\n");

        discard_confirmed(root, "-rf", true, Some(ConfirmedEntry::Folder)).unwrap();
        discard_confirmed(root, "a*b", true, Some(ConfirmedEntry::Folder)).unwrap();

        assert!(!root.join("-rf").exists());
        assert!(!root.join("a*b").exists());
        assert!(root.join("ab/keep.txt").exists());
        assert!(root.join("axb/keep.txt").exists());
    }

    /// A folder the user agreed to clean, which becomes a repository before the
    /// confirm lands (an agent runs `git init` in it), must not be removed
    /// whole: the dialog never said its ignored files or new history would go.
    #[test]
    fn a_folder_that_became_a_repository_after_the_dialog_is_not_deleted() {
        let repo = repo();
        let root = repo.path();
        write(root, ".gitignore", "*.env\n");
        git_in(root)(&["add", ".gitignore"]);
        git_in(root)(&["commit", "-q", "-m", "ignore"]);
        write(root, "scratch/a.js", "a\n");
        write(root, "scratch/secret.env", "SECRET=1\n");
        git_in(&root.join("scratch"))(&["init", "-q"]);

        let refusal =
            discard_confirmed(root, "scratch", true, Some(ConfirmedEntry::Folder)).unwrap_err();

        assert!(refusal.to_string().contains("changed since"), "{refusal}");
        assert!(root.join("scratch/secret.env").exists());
        assert!(root.join("scratch/.git").exists());
        // Nothing named it a repository, so nothing may delete it as one.
        assert!(discard_confirmed(root, "scratch", true, None).is_err());
        assert!(root.join("scratch/.git").exists());
        // Named as what it is, it goes.
        discard_confirmed(root, "scratch", true, Some(ConfirmedEntry::Repository)).unwrap();
        assert!(!root.join("scratch").exists());
    }

    /// The opposite change: confirmed as a repository, now an ordinary folder.
    #[test]
    fn a_repository_that_became_a_folder_after_the_dialog_is_not_cleaned() {
        let repo = repo();
        let root = repo.path();
        write(root, "was-repo/a.js", "a\n");

        let refusal = discard_confirmed(root, "was-repo", true, Some(ConfirmedEntry::Repository))
            .unwrap_err();

        assert!(refusal.to_string().contains("changed since"), "{refusal}");
        assert!(root.join("was-repo/a.js").exists());
    }

    /// A repository of its own at the top keeps the delete it always had: the
    /// row is the repository, and deleting it removes it whole.
    #[test]
    fn discarding_a_nested_repository_row_removes_it_whole() {
        let repo = repo();
        let root = repo.path();
        let nested = root.join("clone");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "a.txt", "a\n");

        discard_confirmed(root, "clone", true, Some(ConfirmedEntry::Repository)).unwrap();

        assert!(!nested.exists());
    }

    /// Deleting a link removes the link, whatever it points at, so the guard
    /// against landing on the worktree or `.git` asks where the LINK is, not
    /// where it leads.
    #[test]
    fn an_untracked_symlink_to_the_worktree_or_into_git_is_still_discardable() {
        let repo = repo();
        let root = repo.path();
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        fs::write(root.join(".git/hooks/keep"), "keep\n").unwrap();
        std::os::unix::fs::symlink(".", root.join("self")).unwrap();
        std::os::unix::fs::symlink(".git/hooks", root.join("hooks")).unwrap();

        discard_file(root, "self", true).unwrap();
        discard_file(root, "hooks", true).unwrap();

        assert!(
            root.join("self").symlink_metadata().is_err(),
            "the link is gone"
        );
        assert!(
            root.join("hooks").symlink_metadata().is_err(),
            "the link is gone"
        );
        assert!(
            root.join("src/lib.rs").exists(),
            "the worktree is untouched"
        );
        assert!(root.join(".git/hooks/keep").exists(), ".git is untouched");
    }

    #[test]
    fn a_tracked_symlink_retargeted_at_git_is_restored_by_discard() {
        let repo = repo();
        let root = repo.path();
        std::os::unix::fs::symlink("src", root.join("link")).unwrap();
        git_in(root)(&["add", "link"]);
        git_in(root)(&["commit", "-q", "-m", "link"]);
        fs::remove_file(root.join("link")).unwrap();
        std::os::unix::fs::symlink(".git", root.join("link")).unwrap();

        discard_file(root, "link", false).unwrap();

        assert_eq!(fs::read_link(root.join("link")).unwrap(), Path::new("src"));
    }

    #[test]
    fn a_path_through_a_link_into_git_is_still_refused() {
        let repo = repo();
        let root = repo.path();
        std::os::unix::fs::symlink(".git", root.join("dotgit")).unwrap();

        assert!(discard_file(root, "dotgit/HEAD", true).is_err());
        assert!(root.join(".git/HEAD").exists());
    }

    /// A folder holding only repositories of their own has nothing a delete
    /// would take, since those are kept: the delete says so instead of
    /// reporting a deletion that did not happen.
    #[test]
    fn deleting_a_folder_that_holds_only_nested_repositories_is_refused() {
        let repo = repo();
        let root = repo.path();
        let nested = root.join("vendor/lib");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "own.txt", "own\n");

        let refusal =
            discard_confirmed(root, "vendor", true, Some(ConfirmedEntry::Folder)).unwrap_err();

        assert!(
            refusal
                .to_string()
                .contains("nothing in \"vendor/\" that a delete would remove"),
            "{refusal}"
        );
        assert!(nested.join("own.txt").exists());
    }

    /// A linked worktree of this same repository placed inside the worktree
    /// looks like a repository of its own, but deleting its directory would
    /// leave the repository believing the worktree still exists. It belongs
    /// to the worktree manager.
    #[test]
    fn a_linked_worktree_of_this_repository_is_not_deleted_as_a_folder() {
        let repo = repo();
        let root = repo.path();
        git_in(root)(&["worktree", "add", "-q", "-b", "side", "inner-wt"]);
        assert_eq!(
            untracked_directory_kind(root, "inner-wt").unwrap(),
            UntrackedDirectoryKind::LinkedWorktree
        );
        let (_, unstaged) = changed_files(root).unwrap();
        assert_eq!(
            shape(&unstaged),
            vec![(
                "inner-wt".to_string(),
                "?".to_string(),
                ChangedFileKind::LinkedWorktree
            )],
            "the listing says what it is, so no surface offers a delete"
        );

        let refusal =
            discard_confirmed(root, "inner-wt", true, Some(ConfirmedEntry::Folder)).unwrap_err();

        assert!(
            refusal.to_string().contains("worktree manager"),
            "{refusal}"
        );
        assert!(root.join("inner-wt/src/lib.rs").exists());
        // An ordinary nested repository is not one.
        let nested = root.join("clone");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        assert_eq!(
            untracked_directory_kind(root, "clone").unwrap(),
            UntrackedDirectoryKind::Repository
        );
    }

    /// Agents work in linked worktrees, so the common directory has to be
    /// found from one as well: a worktree of the repository placed inside an
    /// agent's worktree is recognised there too.
    #[test]
    fn a_linked_worktree_inside_an_agents_worktree_is_recognised() {
        let repo = repo();
        let root = repo.path();
        git_in(root)(&["worktree", "add", "-q", "-b", "agent", "agent-wt"]);
        git_in(root)(&["worktree", "add", "-q", "-b", "other", "agent-wt/inner"]);
        let agent = root.join("agent-wt");

        let (_, unstaged) = changed_files(&agent).unwrap();

        assert_eq!(
            shape(&unstaged),
            vec![(
                "inner".to_string(),
                "?".to_string(),
                ChangedFileKind::LinkedWorktree
            )]
        );
    }

    /// The listing and the delete decide "repository of its own" the same way,
    /// from git. A folder that only looks like one (an empty `.git` directory,
    /// a `.git` file pointing nowhere) is an ordinary folder to git, so it is
    /// listed as one and deleted as one: its ignored files are kept.
    #[test]
    fn a_folder_that_only_looks_like_a_repository_keeps_its_ignored_files() {
        let repo = repo();
        let root = repo.path();
        write(root, ".gitignore", "*.env\n");
        git_in(root)(&["add", ".gitignore"]);
        git_in(root)(&["commit", "-q", "-m", "ignore"]);
        write(root, "empty/a.js", "a\n");
        write(root, "empty/secret.env", "SECRET=1\n");
        fs::create_dir_all(root.join("empty/.git")).unwrap();
        write(root, "stale/a.js", "a\n");
        write(root, "stale/secret.env", "SECRET=1\n");
        write(root, "stale/.git", "gitdir: /nonexistent/place\n");

        let (_, unstaged) = changed_files(root).unwrap();
        for folder in ["empty", "stale"] {
            let row = unstaged.iter().find(|f| f.path == folder).expect(folder);
            assert!(row.is_expandable(), "{folder} lists as an ordinary folder");
            assert_eq!(
                untracked_directory_kind(root, folder).unwrap(),
                UntrackedDirectoryKind::Folder
            );
            discard_confirmed(root, folder, true, Some(ConfirmedEntry::Folder)).unwrap();
            assert!(
                !root.join(folder).join("a.js").exists(),
                "{folder}: its file went"
            );
            assert!(
                root.join(folder).join("secret.env").exists(),
                "{folder}: the ignored file is kept"
            );
        }
    }

    /// A worktree of this same repository is the worktree manager's: staging it
    /// would record a link to it in the index, which is never what was meant.
    #[test]
    fn staging_a_linked_worktree_is_refused() {
        let repo = repo();
        let root = repo.path();
        git_in(root)(&["worktree", "add", "-q", "-b", "side", "inner-wt"]);

        let refusal = stage_file(root, "inner-wt").unwrap_err();
        assert!(
            refusal.to_string().contains("worktree manager"),
            "{refusal}"
        );
        assert!(stage_files(root, &["inner-wt".to_string()]).is_err());

        let (staged, _) = changed_files(root).unwrap();
        assert!(staged.is_empty(), "{staged:?}");
    }

    // ── Paths inside a folder that git does not list ──────────────────────
    //
    // A folder row answers for the files inside it, but only for the ones git
    // itself would list: an ignored file, a file that does not exist and a
    // file inside a repository of its own are not changes, and a discard must
    // not reach them through the folder.

    fn child_repo() -> tempfile::TempDir {
        let repo = repo();
        let root = repo.path();
        write(root, ".gitignore", "*.env\n");
        git_in(root)(&["add", ".gitignore"]);
        git_in(root)(&["commit", "-q", "-m", "ignore"]);
        write(root, "build/app.js", "a\n");
        write(root, "build/sub/b.js", "b\n");
        write(root, "build/secret.env", "SECRET=1\n");
        let nested = root.join("build/lib");
        fs::create_dir_all(&nested).unwrap();
        git_in(&nested)(&["init", "-q"]);
        write(&nested, "own.txt", "own\n");
        repo
    }

    #[test]
    fn discard_refuses_a_path_inside_a_folder_that_git_does_not_list() {
        let repo = child_repo();
        let root = repo.path();
        for path in ["build/secret.env", "build/nope.js", "build/lib/own.txt"] {
            let refusal = discard_classify(root, path).expect_err(path);
            assert!(
                refusal.to_string().contains("is not a change git lists"),
                "{path}: {refusal}"
            );
        }
        assert!(root.join("build/secret.env").exists());
        assert!(root.join("build/lib/own.txt").exists());
        // What git does list inside the folder still answers.
        assert!(discard_classify(root, "build/app.js").unwrap());
        assert!(discard_classify(root, "build/sub").unwrap());
        assert!(discard_classify(root, "build/lib").unwrap());
    }

    #[test]
    fn only_listed_changes_inside_a_folder_answer_on_either_side() {
        let repo = child_repo();
        let root = repo.path();
        let (_, unstaged) = changed_files(root).unwrap();
        let asked: Vec<String> = [
            "build/app.js",
            "build/sub",
            "build/secret.env",
            "build/nope.js",
            "build/lib/own.txt",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        let answered = rows_answering(root, &unstaged, ChangesSide::Unstaged, &asked).unwrap();
        let mut answered: Vec<_> = answered.into_iter().collect();
        answered.sort();
        assert_eq!(
            answered,
            vec!["build/app.js".to_string(), "build/sub".to_string()]
        );

        git_in(root)(&["add", "--", "build/app.js", "build/sub"]);
        let (staged, _) = changed_files(root).unwrap();
        let asked: Vec<String> = ["build/app.js", "build/sub/b.js", "build/sub/none.js"]
            .iter()
            .map(|p| p.to_string())
            .collect();
        let answered = rows_answering(root, &staged, ChangesSide::Staged, &asked).unwrap();
        let mut answered: Vec<_> = answered.into_iter().collect();
        answered.sort();
        assert_eq!(
            answered,
            vec!["build/app.js".to_string(), "build/sub/b.js".to_string()]
        );
    }

    // ── Crafted paths ──────────────────────────────────────────────────────
    //
    // A folded folder answers for the files inside it, and "inside" is decided
    // on the path's text. A path that climbs back out of the folder, or names
    // the folder itself in a roundabout way, must never be taken for one of
    // its files: `node_modules/..` is the worktree root, and discarding it
    // emptied the whole worktree, `.git` included.

    const CRAFTED: &[&str] = &[
        "node_modules/..",
        "node_modules/../README",
        "node_modules/../src/lib.rs",
        "node_modules/./pkg/a.js",
        "node_modules//pkg/a.js",
        "node_modules/pkg/../../README",
        "node_modules/",
        "node_modules\\..\\README",
        "node_modules/..\\README",
        "/node_modules/pkg/a.js",
        "node_modules/../.git",
    ];

    /// A repository with a committed README and `src/lib.rs`, and an
    /// untracked `node_modules` of two files.
    fn crafted_repo() -> tempfile::TempDir {
        let repo = repo();
        let root = repo.path();
        write(root, "README", "readme\n");
        git_in(root)(&["add", "README"]);
        git_in(root)(&["commit", "-q", "-m", "readme"]);
        write(root, "node_modules/pkg/a.js", "a\n");
        write(root, "node_modules/b.js", "b\n");
        repo
    }

    /// Everything a crafted discard could have destroyed is still there.
    fn assert_untouched(root: &Path) {
        assert!(root.join("README").exists(), "README survived");
        assert!(root.join("src/lib.rs").exists(), "src/lib.rs survived");
        assert!(root.join(".git/HEAD").exists(), ".git survived");
        assert!(
            root.join("node_modules/pkg/a.js").exists(),
            "the folder survived"
        );
        let (staged, unstaged) = changed_files(root).unwrap();
        assert!(staged.is_empty(), "nothing got staged: {staged:?}");
        assert_eq!(
            shape(&unstaged),
            vec![("node_modules".to_string(), "?".to_string(), folder(2))]
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_a_folder_is_not_inside_it() {
        let repo = crafted_repo();
        let (_, unstaged) = changed_files(repo.path()).unwrap();
        for path in CRAFTED {
            assert!(
                crate::model::listing_row_for(&unstaged, path).is_none(),
                "{path:?} must not be answered for by the folder"
            );
        }
        // The honest spellings still are.
        assert!(crate::model::listing_row_for(&unstaged, "node_modules/pkg/a.js").is_some());
        assert!(crate::model::listing_row_for(&unstaged, "node_modules").is_some());
    }

    #[test]
    fn a_crafted_path_is_never_classified_for_discard() {
        let repo = crafted_repo();
        for path in CRAFTED {
            assert!(
                discard_classify(repo.path(), path).is_err(),
                "{path:?} must be refused"
            );
        }
        assert_untouched(repo.path());
    }

    #[test]
    fn discard_refuses_a_crafted_path_whatever_it_is_told() {
        let repo = crafted_repo();
        let root = repo.path();
        for path in CRAFTED.iter().chain(&[".", "", ".git", "src/.."]) {
            for untracked in [true, false] {
                assert!(
                    discard_file(root, path, untracked).is_err(),
                    "discarding {path:?} (untracked: {untracked}) must be refused"
                );
            }
        }
        assert_untouched(root);
    }

    #[test]
    fn stage_and_unstage_refuse_a_crafted_path() {
        let repo = crafted_repo();
        let root = repo.path();
        for path in CRAFTED.iter().chain(&[".", "", "src/.."]) {
            if path.contains('\\') {
                // A backslash is an ordinary name character here, so git reads
                // these as one literal name that does not exist: whatever git
                // answers, nothing else in the tree can be reached through it,
                // which `assert_untouched` below checks.
                let _ = stage_file(root, path);
                let _ = unstage_file(root, path);
                continue;
            }
            assert!(
                stage_file(root, path).is_err(),
                "staging {path:?} is refused"
            );
            assert!(
                unstage_file(root, path).is_err(),
                "unstaging {path:?} is refused"
            );
            assert!(
                stage_files(root, &[path.to_string()]).is_err(),
                "batch-staging {path:?} is refused"
            );
        }
        assert_untouched(root);
    }
}
