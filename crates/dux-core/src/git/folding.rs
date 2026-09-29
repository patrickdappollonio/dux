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
    /// Its untracked, not-ignored files, and the repositories of their own it
    /// holds.
    Contents(crate::model::FolderContents),
    /// The folder is a repository of its own, which git does not look inside.
    NestedRepository,
}

impl UntrackedFolder {
    pub(super) fn kind(self) -> ChangedFileKind {
        match self {
            Self::Contents(contents) => ChangedFileKind::Directory(contents),
            Self::NestedRepository => ChangedFileKind::NestedRepository,
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
    /// Repositories of their own inside it, at any depth.
    nested_repositories: usize,
}

impl<'r> Tally<'r> {
    /// Count one record found inside the folder. A record ending in `/` is a
    /// repository git did not look inside, and it is not a file.
    fn add(&mut self, record: &'r [u8]) {
        if record.ends_with(b"/") {
            self.nested_repositories += 1;
        } else {
            self.files.push(record);
        }
    }
}

/// Turn the tallies of one read into what each folder holds, fingerprinting
/// folders in order for as long as the read's stat budget lasts. A folder that
/// would not fit in what is left gets no fingerprint at all rather than a
/// partial one, which could miss exactly the file that changed.
fn finish_tallies(
    worktree: &Path,
    tallies: Vec<(String, Tally<'_>)>,
) -> Vec<(String, UntrackedFolder)> {
    let mut budget = UNTRACKED_STATS_MAX_FILES;
    tallies
        .into_iter()
        .map(|(folder, tally)| {
            if tally.is_repository {
                return (folder, UntrackedFolder::NestedRepository);
            }
            let fingerprint = (tally.files.len() <= budget).then(|| {
                budget -= tally.files.len();
                fingerprint_files(worktree, &tally.files)
            });
            let contents = crate::model::FolderContents {
                file_count: tally.files.len(),
                nested_repositories: tally.nested_repositories,
                fingerprint,
            };
            (folder, UntrackedFolder::Contents(contents))
        })
        .collect()
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
    let raw = untracked_records(worktree, None)?;
    for record in raw.split(|byte| *byte == 0).filter(|r| !r.is_empty()) {
        // The folder itself answering as `folder/` is a repository of its own.
        if let Some(&position) = record.strip_suffix(b"/").and_then(|e| index.get(e)) {
            tallies[position].1.is_repository = true;
            continue;
        }
        let owner = record
            .iter()
            .enumerate()
            .filter(|(position, byte)| **byte == b'/' && *position + 1 < record.len())
            .find_map(|(position, _)| index.get(&record[..position]).copied());
        if let Some(position) = owner {
            tallies[position].1.add(record);
        }
    }
    Ok(finish_tallies(worktree, tallies).into_iter().collect())
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
/// The folder must also have been staged WHOLE: anything untracked inside it
/// (a file, or a folded folder, in `unstaged`) means only part of it is in the
/// index, and a folder row would claim the rest. A tracked change inside it
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

    // The fold root of each added file: its shallowest ancestor HEAD lacks.
    let root_of: Vec<Option<String>> = staged
        .iter()
        .map(|file| {
            if file.status != "A" {
                return None;
            }
            ancestors(&file.path)
                .find(|ancestor| missing.contains(*ancestor))
                .map(str::to_string)
        })
        .collect();
    let mut roots: HashMap<&str, usize> = HashMap::new();
    for root in root_of.iter().flatten() {
        *roots.entry(root.as_str()).or_default() += 1;
    }
    // A root holding anything other than a plain addition stays open.
    for file in staged.iter().filter(|file| file.status != "A") {
        for ancestor in ancestors(&file.path) {
            roots.remove(ancestor);
        }
    }
    // So does one staged in part: something inside it is still untracked.
    for file in unstaged.iter().filter(|file| file.status == "?") {
        roots.remove(file.path.as_str());
        for ancestor in ancestors(&file.path) {
            roots.remove(ancestor);
        }
    }
    if roots.is_empty() {
        return staged;
    }
    let roots: HashMap<String, usize> = roots
        .into_iter()
        .map(|(root, count)| (root.to_string(), count))
        .collect();

    let mut folded = Vec::with_capacity(staged.len());
    let mut emitted: HashSet<String> = HashSet::new();
    for (file, root) in staged.into_iter().zip(root_of) {
        match root.filter(|root| roots.contains_key(root)) {
            Some(root) => {
                if !emitted.contains(&root) {
                    let file_count = roots[&root];
                    emitted.insert(root.clone());
                    folded.push(folder_row(root, "A", file_count));
                }
            }
            None => folded.push(file),
        }
    }
    folded
}

/// A folded folder row.
fn folder_row(path: String, status: &str, file_count: usize) -> ChangedFile {
    ChangedFile {
        status: status.to_string(),
        path,
        additions: 0,
        deletions: 0,
        binary: false,
        diff_excluded: false,
        renamed_from: None,
        kind: ChangedFileKind::directory(file_count),
    }
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
        finish_tallies(worktree, folders)
            .into_iter()
            .map(|(child, folder)| ChangedFile {
                status: "?".to_string(),
                path: format!("{prefix}{child}"),
                additions: 0,
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

    let mut files: Vec<ChangedFile> = Vec::new();
    let mut folders: Vec<(String, usize)> = Vec::new();
    let mut folder_index: HashMap<String, usize> = HashMap::new();
    for entry in parse_status_porcelain_z(&output.stdout) {
        if matches!(entry.index_status, ' ' | '?') {
            continue;
        }
        let Some(rest) = entry.path.strip_prefix(&pathspec) else {
            continue;
        };
        let (child, deeper) = split_child(rest.as_bytes());
        let child = String::from_utf8_lossy(child).into_owned();
        if deeper {
            let slot = *folder_index.entry(child.clone()).or_insert_with(|| {
                folders.push((child.clone(), 0));
                folders.len() - 1
            });
            folders[slot].1 += 1;
        } else {
            files.push(ChangedFile {
                status: entry.index_status.to_string(),
                path: entry.path.clone(),
                additions: 0,
                deletions: 0,
                binary: false,
                diff_excluded: false,
                renamed_from: rename_source(entry.index_status, &entry.renamed_from),
                kind: ChangedFileKind::File,
            });
        }
    }

    if !files.is_empty() {
        let numstat = Command::new("git")
            .args([
                "--literal-pathspecs",
                "-C",
                wt.as_ref(),
                "diff",
                "--cached",
                "--numstat",
                "-z",
                "--",
                &pathspec,
            ])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| parse_numstat(&out.stdout))
            .unwrap_or_default();
        let attribute_unset = paths_excluded_from_diffs(wt.as_ref(), &countless_paths(&[&numstat]));
        let excluded = diff_excluded_rows(worktree, &attribute_unset, &numstat, ContentSide::Index);
        for file in &mut files {
            if let Some(stat) = numstat.get(&file.path) {
                apply_stat(file, stat, &excluded);
            }
        }
    }

    files.extend(
        folders
            .into_iter()
            .map(|(child, count)| folder_row(format!("{pathspec}{child}"), "A", count)),
    );
    Ok(files)
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
            fingerprint: None,
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

        discard_file(root, "dist", true).unwrap();
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
        discard_file(root, "config", true).unwrap();

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

        discard_file(root, "-rf", true).unwrap();
        discard_file(root, "a*b", true).unwrap();

        assert!(!root.join("-rf").exists());
        assert!(!root.join("a*b").exists());
        assert!(root.join("ab/keep.txt").exists());
        assert!(root.join("axb/keep.txt").exists());
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

        discard_file(root, "clone", true).unwrap();

        assert!(!nested.exists());
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
