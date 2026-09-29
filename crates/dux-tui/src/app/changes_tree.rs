//! Folded folders in the changes pane.
//!
//! The changed-files listing reports a wholly untracked folder (and a folder
//! staged whole) as ONE row with a file count, the way `git status` does. This
//! module is the terminal UI's half of that: the rows the pane shows once some
//! of those folders are expanded, the workers that list a folder's contents a
//! level at a time, and the workers that stage, unstage or delete a whole
//! folder. Every one of those runs off the UI thread, because a folder can hold
//! tens of thousands of files.
//!
//! Expanded folders are remembered per agent for as long as dux runs, and are
//! re-listed quietly whenever the pane's own read says a folder's file count
//! moved, or dropped when the folder is no longer a row at all.

use super::*;
use dux_core::git::ChangesSide;
use dux_core::model::{ChangedFileKind, folder_count_label, total_file_count};

/// One row of the changes pane, flattened from the listing and its expanded
/// folders.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ChangesRow<'a> {
    /// A file or folder. `depth` is how many expanded folders it sits inside.
    Entry {
        file: &'a ChangedFile,
        depth: usize,
        expanded: bool,
    },
    /// An expanded folder whose contents a worker is still listing.
    Loading { depth: usize },
    /// An expanded folder whose listing failed.
    Failed { depth: usize, message: &'a str },
}

impl<'a> ChangesRow<'a> {
    /// The file or folder this row stands for, `None` for a placeholder row.
    pub(crate) fn file(&self) -> Option<&'a ChangedFile> {
        match self {
            Self::Entry { file, .. } => Some(file),
            Self::Loading { .. } | Self::Failed { .. } => None,
        }
    }
}

/// Which half of the changes pane a section is, `None` for the commit box.
pub(crate) fn changes_side(section: RightSection) -> Option<ChangesSide> {
    match section {
        RightSection::Staged => Some(ChangesSide::Staged),
        RightSection::Unstaged => Some(ChangesSide::Unstaged),
        RightSection::CommitInput => None,
    }
}

/// How a folder is named on screen: its path with a trailing slash, which is
/// what tells it apart from a file of the same name.
pub(crate) fn folder_label(file: &ChangedFile) -> String {
    if file.is_folder() {
        format!("{}/", file.path)
    } else {
        file.path.clone()
    }
}

/// The status key a folder listing's busy and final share, so expanding the
/// same folder again replaces its message rather than stacking a second one.
fn listing_status_key(session_id: &str, side: ChangesSide, dir: &str) -> String {
    format!("changes-folder:{session_id}:{side:?}:{dir}")
}

/// The status key a whole-folder operation's busy and final share.
fn op_status_key(session_id: &str, dir: &str) -> String {
    format!("changes-folder-op:{session_id}:{dir}")
}

/// Where one expanded folder's own row is, for [`App::reconcile_changes_tree`].
enum RowLookup<'a> {
    Found(&'a ChangedFile),
    /// Its parent folder is being listed, so there is no answer yet.
    Unknown,
    Gone,
}

/// Every row of one section, with expanded folders' contents under them.
fn push_rows<'a>(
    rows: &mut Vec<ChangesRow<'a>>,
    files: &'a [ChangedFile],
    expanded: Option<&'a HashMap<String, FolderListing>>,
    depth: usize,
) {
    for file in files {
        let listing = expanded
            .filter(|_| file.is_expandable())
            .and_then(|map| map.get(&file.path));
        rows.push(ChangesRow::Entry {
            file,
            depth,
            expanded: listing.is_some(),
        });
        let Some(listing) = listing else {
            continue;
        };
        if let Some(message) = listing.error.as_deref() {
            rows.push(ChangesRow::Failed {
                depth: depth + 1,
                message,
            });
        } else if let Some(children) = listing.children.as_deref() {
            push_rows(rows, children, expanded, depth + 1);
        } else {
            rows.push(ChangesRow::Loading { depth: depth + 1 });
        }
    }
}

impl App {
    /// The listing one half of the pane shows, before any folder is expanded.
    fn side_files(&self, side: ChangesSide) -> &[ChangedFile] {
        match side {
            ChangesSide::Staged => &self.engine.staged_files,
            ChangesSide::Unstaged => &self.engine.unstaged_files,
        }
    }

    /// The expanded folders of the selected agent, for one half of the pane.
    fn expanded_folders(&self, side: ChangesSide) -> Option<&HashMap<String, FolderListing>> {
        let session = self.selected_session()?;
        self.changes_tree
            .by_session
            .get(&session.id)?
            .expanded
            .get(&side)
    }

    /// The rows the pane shows for `section`: the listing, with every expanded
    /// folder's contents under it. `files_index` indexes this.
    pub(crate) fn changes_rows(&self, section: RightSection) -> Vec<ChangesRow<'_>> {
        let Some(side) = changes_side(section) else {
            return Vec::new();
        };
        let files = self.side_files(side);
        let mut rows = Vec::with_capacity(files.len());
        push_rows(&mut rows, files, self.expanded_folders(side), 0);
        rows
    }

    /// The row the cursor is on.
    pub(crate) fn selected_changes_row(&self) -> Option<ChangesRow<'_>> {
        self.changes_rows(self.right_section)
            .into_iter()
            .nth(self.files_index)
    }

    /// `Some(expanded)` when the cursor is on a folder that can be expanded,
    /// which is what decides the Enter key's word in the hints.
    pub(crate) fn selected_folder_expanded(&self) -> Option<bool> {
        match self.selected_changes_row()? {
            ChangesRow::Entry { file, expanded, .. } if file.is_expandable() => Some(expanded),
            _ => None,
        }
    }

    /// Say what the diff key does on the row under the cursor: on a folder it
    /// expands or collapses, and on a repository of its own it does nothing,
    /// so its hint is dropped rather than promising a diff. The key itself is
    /// whatever the binding says, found by the action rather than by name.
    pub(crate) fn name_the_folder_key(&self, hints: &mut Vec<(String, &'static str)>) {
        let diff_key = self.bindings.label_for(Action::OpenDiff);
        let Some(position) = hints.iter().position(|(key, _)| *key == diff_key) else {
            return;
        };
        match self.selected_changed_file().map(|file| &file.kind) {
            Some(ChangedFileKind::Directory { .. }) => {
                let expanded = self.selected_folder_expanded().unwrap_or(false);
                hints[position].1 = if expanded { "Collapse" } else { "Expand" };
            }
            Some(ChangedFileKind::NestedRepository | ChangedFileKind::LinkedWorktree) => {
                hints.remove(position);
            }
            Some(ChangedFileKind::File) | None => {}
        }
    }

    /// Enter on a folder row: expand it, or collapse it when it is expanded.
    /// Answers `false` when the cursor is on a file, so the caller opens its
    /// diff instead. A folder never gets a diff: that would read a directory as
    /// a file.
    pub(crate) fn toggle_selected_folder(&mut self) -> bool {
        let Some(row) = self.selected_changes_row() else {
            return false;
        };
        let (path, kind, expanded) = match row {
            ChangesRow::Entry { file, expanded, .. } if file.is_folder() => {
                (file.path.clone(), file.kind.clone(), expanded)
            }
            ChangesRow::Entry { .. } => return false,
            // A placeholder row stands for nothing to open.
            ChangesRow::Loading { .. } | ChangesRow::Failed { .. } => return true,
        };
        let Some(side) = changes_side(self.right_section) else {
            return true;
        };
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return true;
        };
        let seen = match kind {
            ChangedFileKind::Directory(contents) => contents,
            ChangedFileKind::NestedRepository => {
                self.set_info(format!(
                    "\"{path}/\" is a repository of its own, so git does not look inside it and \
                     there is nothing of this worktree's in it to list. Its row stages, unstages \
                     and deletes it whole."
                ));
                return true;
            }
            ChangedFileKind::LinkedWorktree => {
                self.set_info(format!(
                    "\"{path}/\" is a worktree of this same repository, so its changes are its \
                     own and git does not list them here. Manage it from the worktree manager."
                ));
                return true;
            }
            ChangedFileKind::File => return false,
        };
        if expanded {
            self.collapse_folder(&session_id, side, &path);
            return true;
        }
        // Listing a folder is a read of the worktree, through the same gate the
        // diff uses.
        let Some(worktree) = self.diff_worktree_for_selection() else {
            return true;
        };
        let seq = self.spawn_folder_listing(&session_id, worktree, side, &path, true);
        self.changes_tree
            .by_session
            .entry(session_id)
            .or_default()
            .expanded
            .entry(side)
            .or_default()
            .insert(
                path,
                FolderListing {
                    children: None,
                    pending_seq: Some(seq),
                    error: None,
                    seen,
                },
            );
        true
    }

    /// Forget an expanded folder and every folder expanded inside it.
    fn collapse_folder(&mut self, session_id: &str, side: ChangesSide, dir: &str) {
        let Some(map) = self
            .changes_tree
            .by_session
            .get_mut(session_id)
            .and_then(|tree| tree.expanded.get_mut(&side))
        else {
            return;
        };
        let inside = format!("{dir}/");
        map.retain(|path, _| path != dir && !path.starts_with(&inside));
        self.clamp_files_cursor();
    }

    /// Ask a worker for one folder's contents. `announce` raises a keyed busy
    /// and a final for it; a quiet refresh of a folder already on screen does
    /// neither, because the rows it replaces are still there to read.
    fn spawn_folder_listing(
        &mut self,
        session_id: &str,
        worktree: PathBuf,
        side: ChangesSide,
        dir: &str,
        announce: bool,
    ) -> u64 {
        self.changes_tree.listing_seq = self.changes_tree.listing_seq.wrapping_add(1);
        let seq = self.changes_tree.listing_seq;
        let status_key = announce.then(|| listing_status_key(session_id, side, dir));
        if let Some(key) = &status_key {
            self.engine.register_status_key(key);
            self.status.set(
                Instant::now(),
                Some(key.clone()),
                StatusTone::Busy,
                format!("Listing what is inside \"{dir}/\"\u{2026}"),
            );
        }
        let (tx, rx) = mpsc::channel();
        let worker_dir = dir.to_string();
        // A thread that fails to start drops `tx`, which the drain reads as a
        // failed listing, so the busy above still reaches a final.
        let _ = thread::Builder::new()
            .name("dux-changes-folder".into())
            .spawn(move || {
                let answer = git::changed_dir_children(&worktree, &worker_dir, side)
                    .map_err(|err| format!("{err:#}"));
                let _ = tx.send(answer);
            });
        self.changes_tree
            .pending_listings
            .push(PendingFolderListing {
                session_id: session_id.to_string(),
                side,
                dir: dir.to_string(),
                seq,
                status_key,
                rx,
            });
        seq
    }

    /// Fold every finished folder listing and folder operation into the pane.
    /// Called once per run-loop tick with the other drains.
    pub(crate) fn drain_changes_tree_work(&mut self) {
        // The row under the cursor, by path: a listing that lands can add or
        // remove rows above it, and the cursor follows the row, not its index.
        let anchor = self.selected_changed_file().map(|file| file.path.clone());
        let listed = self.drain_folder_listings();
        let operated = self.drain_folder_ops();
        if listed || operated {
            self.mark_frame_dirty();
        }
        if listed {
            self.reconcile_changes_tree();
            if let Some(path) = anchor
                && let Some(index) = self
                    .changes_rows(self.right_section)
                    .iter()
                    .position(|row| row.file().is_some_and(|file| file.path == path))
            {
                self.files_index = index;
            }
            self.clamp_files_cursor();
        }
    }

    fn drain_folder_listings(&mut self) -> bool {
        let mut landed = Vec::new();
        let mut index = 0;
        while index < self.changes_tree.pending_listings.len() {
            let answer = match self.changes_tree.pending_listings[index].rx.try_recv() {
                Ok(answer) => answer,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("the worker listing it stopped before answering".to_string())
                }
            };
            let pending = self.changes_tree.pending_listings.remove(index);
            landed.push((pending, answer));
        }
        let any = !landed.is_empty();
        for (pending, answer) in landed {
            self.apply_folder_listing(pending, answer);
        }
        any
    }

    fn apply_folder_listing(
        &mut self,
        pending: PendingFolderListing,
        answer: Result<Vec<ChangedFile>, String>,
    ) {
        let entry = self
            .changes_tree
            .by_session
            .get_mut(&pending.session_id)
            .and_then(|tree| tree.expanded.get_mut(&pending.side))
            .and_then(|map| map.get_mut(&pending.dir))
            .filter(|entry| entry.pending_seq == Some(pending.seq));
        let Some(entry) = entry else {
            // Collapsed, or asked for again, while the worker ran: nothing on
            // screen is waiting for this answer, so retire its spinner quietly.
            if let Some(key) = &pending.status_key {
                self.status.clear(key, None);
            }
            return;
        };
        entry.pending_seq = None;
        let label = format!("{}/", pending.dir);
        let message = match answer {
            Ok(children) => {
                let folders = children.iter().filter(|c| c.is_folder()).count();
                let files = children.len() - folders;
                let total = total_file_count(&children);
                entry.children = Some(children);
                entry.error = None;
                (
                    StatusTone::Info,
                    format!(
                        "Expanded \"{label}\": {} and {} directly inside, {} in all.",
                        plural(folders, "folder", "folders"),
                        plural(files, "file", "files"),
                        plural(total, "file", "files"),
                    ),
                )
            }
            Err(err) => {
                entry.error = Some(err.clone());
                (
                    StatusTone::Error,
                    format!(
                        "Could not list what is inside \"{label}\": {}. Collapse it and expand \
                         it again to retry.",
                        err.trim().trim_end_matches('.')
                    ),
                )
            }
        };
        if let Some(key) = pending.status_key {
            self.status
                .set(Instant::now(), Some(key), message.0, message.1);
        }
    }

    /// Bring the selected agent's expanded folders in line with the listing
    /// the pane now shows. A folder that is no longer a folded row is
    /// forgotten; one whose file count moved is listed again, quietly, with
    /// its old contents left on screen until the new ones land. Called after
    /// every changed-files read the pane applies and after every listing.
    pub(crate) fn reconcile_changes_tree(&mut self) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        if self.changes_tree.lists_for.as_deref() != Some(session_id.as_str()) {
            return;
        }
        let Some(tree) = self.changes_tree.by_session.get(&session_id) else {
            return;
        };
        let mut drops: Vec<(ChangesSide, String)> = Vec::new();
        let mut refreshes: Vec<(ChangesSide, String, dux_core::model::FolderContents)> = Vec::new();
        for (side, map) in &tree.expanded {
            let top = self.side_files(*side);
            for (dir, listing) in map {
                match find_folder_row(top, map, dir) {
                    RowLookup::Found(row) if row.is_expandable() => {
                        if let Some(now) = row.folder_contents()
                            && *now != listing.seen
                            && listing.pending_seq.is_none()
                        {
                            refreshes.push((*side, dir.clone(), now.clone()));
                        }
                    }
                    RowLookup::Found(_) | RowLookup::Gone => drops.push((*side, dir.clone())),
                    RowLookup::Unknown => {}
                }
            }
        }
        for (side, dir) in drops {
            self.collapse_folder(&session_id, side, &dir);
        }
        if refreshes.is_empty() {
            return;
        }
        let Some(worktree) = self
            .engine
            .session_git_access(&session_id)
            .filter(|access| access.changes_panel_works())
            .map(|access| access.directory().to_path_buf())
        else {
            return;
        };
        for (side, dir, seen) in refreshes {
            let seq = self.spawn_folder_listing(&session_id, worktree.clone(), side, &dir, false);
            if let Some(entry) = self
                .changes_tree
                .by_session
                .get_mut(&session_id)
                .and_then(|tree| tree.expanded.get_mut(&side))
                .and_then(|map| map.get_mut(&dir))
            {
                entry.pending_seq = Some(seq);
                entry.seen = seen;
            }
        }
    }

    /// Stage, unstage or delete a whole folder on a worker, with a keyed busy
    /// and a final that says what happened. The changed files are read again
    /// once it finishes.
    pub(crate) fn start_folder_op(&mut self, op: FolderOp, folder: &ChangedFile) {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select an agent first: a folder belongs to an agent's worktree.");
            return;
        };
        let Some(worktree) = self.changes_worktree_for_selection() else {
            return;
        };
        let label = folder_label(folder);
        let count_words = folder_count_label(folder).unwrap_or_else(|| "1 file".to_string());
        let status_key = op_status_key(&session_id, &folder.path);
        if self
            .changes_tree
            .pending_ops
            .iter()
            .any(|pending| pending.status_key == status_key)
        {
            self.set_info(format!(
                "\"{label}\" is already being changed. Wait for that to finish before asking again."
            ));
            return;
        }
        let busy = match op {
            FolderOp::Stage => format!("Staging \"{label}\" ({count_words})\u{2026}"),
            FolderOp::Unstage => format!("Unstaging \"{label}\" ({count_words})\u{2026}"),
            FolderOp::Delete => format!("Deleting \"{label}\" ({count_words})\u{2026}"),
        };
        self.engine.register_status_key(&status_key);
        self.status.set(
            Instant::now(),
            Some(status_key.clone()),
            StatusTone::Busy,
            busy,
        );

        let (tx, rx) = mpsc::channel();
        let path = folder.path.clone();
        let _ = thread::Builder::new()
            .name("dux-changes-folder-op".into())
            .spawn(move || {
                let outcome = match op {
                    FolderOp::Stage => git::stage_file(&worktree, &path),
                    FolderOp::Unstage => git::unstage_file(&worktree, &path),
                    FolderOp::Delete => delete_untracked_folder(&worktree, &path),
                };
                let _ = tx.send(outcome.map_err(|err| format!("{err:#}")));
            });
        self.changes_tree.pending_ops.push(PendingFolderOp {
            session_id,
            op,
            label,
            count_words,
            repository: folder.kind == ChangedFileKind::NestedRepository,
            status_key,
            rx,
        });
    }

    fn drain_folder_ops(&mut self) -> bool {
        let mut landed = Vec::new();
        let mut index = 0;
        while index < self.changes_tree.pending_ops.len() {
            let outcome = match self.changes_tree.pending_ops[index].rx.try_recv() {
                Ok(outcome) => outcome,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("the worker running it stopped before answering".to_string())
                }
            };
            landed.push((self.changes_tree.pending_ops.remove(index), outcome));
        }
        let any = !landed.is_empty();
        let mut reload = false;
        for (pending, outcome) in landed {
            let label = &pending.label;
            let count = &pending.count_words;
            let (tone, message) = match (pending.op, outcome) {
                (FolderOp::Stage, Ok(())) if pending.repository => (
                    StatusTone::Info,
                    format!(
                        "Staged \"{label}\" as a link to the repository inside it (a \
                         submodule-style entry recording its current commit), not as its files."
                    ),
                ),
                (FolderOp::Stage, Ok(())) => (
                    StatusTone::Info,
                    format!(
                        "Staged \"{label}\" ({count}): the whole folder is in the staged changes \
                         now."
                    ),
                ),
                (FolderOp::Unstage, Ok(())) => (
                    StatusTone::Info,
                    format!(
                        "Unstaged \"{label}\" ({count}): the whole folder is back in the unstaged \
                         changes."
                    ),
                ),
                (FolderOp::Delete, Ok(())) if pending.repository => (
                    StatusTone::Info,
                    format!(
                        "Deleted \"{label}\", a repository of its own, with its history. This \
                         cannot be undone."
                    ),
                ),
                (FolderOp::Delete, Ok(())) => (
                    StatusTone::Info,
                    format!(
                        "Deleted the untracked files in \"{label}\" ({count}). Files the \
                         repository ignores and repositories of their own inside it are kept. \
                         This cannot be undone."
                    ),
                ),
                (op, Err(err)) => {
                    let verb = match op {
                        FolderOp::Stage => "stage",
                        FolderOp::Unstage => "unstage",
                        FolderOp::Delete => "delete",
                    };
                    (
                        StatusTone::Error,
                        format!(
                            "Could not {verb} \"{label}\": {}.",
                            err.trim().trim_end_matches('.')
                        ),
                    )
                }
            };
            self.status
                .set(Instant::now(), Some(pending.status_key), tone, message);
            reload |= self
                .selected_session()
                .is_some_and(|session| session.id == pending.session_id);
        }
        if reload {
            self.reload_changed_files();
        }
        any
    }
}

/// Find the row an expanded folder hangs from: a row of the listing itself,
/// or a row inside its parent folder's listing.
fn find_folder_row<'a>(
    top: &'a [ChangedFile],
    expanded: &'a HashMap<String, FolderListing>,
    dir: &str,
) -> RowLookup<'a> {
    if let Some(row) = top.iter().find(|file| file.path == dir) {
        return RowLookup::Found(row);
    }
    let Some((parent, _)) = dir.rsplit_once('/') else {
        return RowLookup::Gone;
    };
    match expanded.get(parent) {
        Some(listing) => match listing.children.as_deref() {
            Some(children) => children
                .iter()
                .find(|file| file.path == dir)
                .map_or(RowLookup::Gone, RowLookup::Found),
            None => RowLookup::Unknown,
        },
        None => RowLookup::Gone,
    }
}

/// Delete a folder only if live status still calls it untracked. The row was
/// classified when the dialog opened, and an agent may have staged or
/// committed inside it since: deleting what is now tracked would destroy work
/// that exists in no other place.
fn delete_untracked_folder(worktree: &Path, path: &str) -> anyhow::Result<()> {
    if !git::discard_classify(worktree, path)? {
        anyhow::bail!(
            "it is no longer untracked, so dux left it alone; refresh the changes and look again"
        );
    }
    git::discard_file(worktree, path, true)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    let noun = if count == 1 { one } else { many };
    format!("{} {noun}", dux_core::model::group_thousands(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, run_git, test_app};
    use ratatui::{Terminal, backend::TestBackend};

    fn folder(path: &str, status: &str, file_count: usize) -> ChangedFile {
        ChangedFile {
            status: status.to_string(),
            path: path.to_string(),
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: ChangedFileKind::directory(file_count),
        }
    }

    fn file(path: &str, status: &str) -> ChangedFile {
        ChangedFile {
            status: status.to_string(),
            path: path.to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: ChangedFileKind::File,
        }
    }

    /// An app whose selected agent's worktree is a real repository holding an
    /// untracked `node_modules` of 13 files (three packages of four, and one
    /// file at the top) next to an untracked `notes.md`.
    fn repo_app() -> (App, PathBuf) {
        let mut app = test_app(default_bindings());
        let worktree = PathBuf::from(
            app.engine.sessions[0]
                .managed_worktree()
                .expect("managed test session"),
        );
        std::fs::create_dir_all(&worktree).unwrap();
        run_git(&worktree, &["init", "-q", "-b", "main"]);
        run_git(&worktree, &["config", "user.name", "test"]);
        run_git(&worktree, &["config", "user.email", "t@t"]);
        std::fs::write(worktree.join("README"), "readme\n").unwrap();
        run_git(&worktree, &["add", "README"]);
        run_git(&worktree, &["commit", "-q", "-m", "init"]);
        for index in 0..12 {
            let dir = worktree.join(format!("node_modules/pkg{}", index / 4));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("f{index}.js")), "x\n").unwrap();
        }
        std::fs::write(worktree.join("node_modules/top.js"), "top\n").unwrap();
        std::fs::write(worktree.join("notes.md"), "notes\n").unwrap();
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;
        app.files_index = 0;
        app.right_hidden = false;
        load_lists(&mut app, &worktree);
        (app, worktree)
    }

    /// Apply a fresh changed-files read the way the pane does when one lands.
    fn load_lists(app: &mut App, worktree: &Path) {
        let (staged, unstaged) = git::changed_files(worktree).expect("changed files");
        app.engine.staged_files = staged;
        app.engine.unstaged_files = unstaged;
        app.changes_tree.lists_for = app.selected_session().map(|s| s.id.clone());
        app.reconcile_changes_tree();
        app.clamp_files_cursor();
    }

    /// Drain folder work until `done` holds, or fail after a few seconds.
    fn settle(app: &mut App, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            app.drain_changes_tree_work();
            if done(app) {
                return;
            }
            assert!(Instant::now() < deadline, "folder work never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn idle(app: &App) -> bool {
        app.changes_tree.pending_listings.is_empty() && app.changes_tree.pending_ops.is_empty()
    }

    /// Each row of a section in a compact form: indent, marker, name.
    fn describe(app: &App, section: RightSection) -> Vec<String> {
        app.changes_rows(section)
            .into_iter()
            .map(|row| match row {
                ChangesRow::Entry {
                    file,
                    depth,
                    expanded,
                } => {
                    let marker = match (&file.kind, expanded) {
                        (ChangedFileKind::Directory { .. }, true) => "v ",
                        (ChangedFileKind::Directory { .. }, false) => "> ",
                        _ => "",
                    };
                    format!(
                        "{}{marker}{} {}",
                        "  ".repeat(depth),
                        folder_label(file),
                        file.file_count()
                    )
                }
                ChangesRow::Loading { depth } => format!("{}loading", "  ".repeat(depth)),
                ChangesRow::Failed { depth, .. } => format!("{}failed", "  ".repeat(depth)),
            })
            .collect()
    }

    fn render_text(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn space() -> KeyEvent {
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)
    }

    fn hint_word(app: &App) -> Option<&'static str> {
        let enter = app.bindings.label_for(Action::OpenDiff);
        app.footer_hints_for(HintContext::Files)
            .into_iter()
            .find(|(key, _)| *key == enter)
            .map(|(_, word)| word)
    }

    #[test]
    fn a_folded_folder_renders_as_one_row_with_its_count() {
        let mut app = test_app(default_bindings());
        app.engine.unstaged_files =
            vec![folder("node_modules", "?", 28_747), file("notes.md", "?")];
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;
        // Selected, so the row is drawn unabridged in this narrow pane.
        app.files_index = 0;
        app.right_hidden = false;

        let screen = render_text(&mut app, 140, 40);

        let row = screen
            .iter()
            .find(|line| line.contains("node_modules/"))
            .unwrap_or_else(|| panic!("the folder renders a row:\n{}", screen.join("\n")));
        assert!(row.contains("? \u{25b8} node_modules/"), "{row}");
        assert!(row.contains("28,747 files"), "{row}");
        let title = screen
            .iter()
            .find(|line| line.contains("Changes ("))
            .expect("the section has a title");
        assert!(
            title.contains("Changes (28748)"),
            "the total counts the files inside the folder: {title}"
        );
    }

    #[test]
    fn enter_expands_a_folder_a_level_at_a_time_and_collapses_it_again() {
        let (mut app, _worktree) = repo_app();
        assert_eq!(
            describe(&app, RightSection::Unstaged),
            vec!["> node_modules/ 13", "notes.md 1"]
        );
        assert_eq!(hint_word(&app), Some("Expand"));

        app.handle_key(enter()).unwrap();
        assert_eq!(
            describe(&app, RightSection::Unstaged),
            vec!["v node_modules/ 13", "  loading", "notes.md 1"],
            "the folder shows it is being listed until the worker answers"
        );
        assert_eq!(app.status.tone(), StatusTone::Busy);
        assert!(
            !matches!(app.center_mode, CenterMode::Diff { .. }),
            "a folder never opens a diff"
        );

        settle(&mut app, idle);
        assert_eq!(
            describe(&app, RightSection::Unstaged),
            vec![
                "v node_modules/ 13",
                "  > node_modules/pkg0/ 4",
                "  > node_modules/pkg1/ 4",
                "  > node_modules/pkg2/ 4",
                "  node_modules/top.js 1",
                "notes.md 1",
            ]
        );
        assert_eq!(app.status.tone(), StatusTone::Info);
        assert!(
            app.status.text().contains("node_modules/"),
            "{}",
            app.status.text()
        );
        assert_eq!(hint_word(&app), Some("Collapse"));

        // One level further down.
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);
        let rows = describe(&app, RightSection::Unstaged);
        assert_eq!(rows[1], "  v node_modules/pkg0/ 4");
        assert_eq!(rows[2], "    node_modules/pkg0/f0.js 1");
        assert_eq!(rows.len(), 10, "{rows:?}");

        // On a file inside, the key is the diff again.
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        assert_eq!(hint_word(&app), Some("Diff"));

        // Collapsing the top folder forgets what was expanded inside it.
        app.files_index = 0;
        app.handle_key(enter()).unwrap();
        assert_eq!(
            describe(&app, RightSection::Unstaged),
            vec!["> node_modules/ 13", "notes.md 1"]
        );
        assert!(
            app.changes_tree.by_session["session-1"].expanded[&ChangesSide::Unstaged].is_empty()
        );
    }

    #[test]
    fn an_expanded_folder_renders_its_contents_indented_under_it() {
        let (mut app, _worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);

        let screen = render_text(&mut app, 140, 40);

        let folder_row = screen
            .iter()
            .position(|line| line.contains("\u{25be} node_modules/"))
            .expect("the expanded folder shows the open marker");
        let child = &screen[folder_row + 1];
        assert!(child.contains("  \u{25b8} pkg0/"), "{child}");
        assert!(child.contains("4 files"), "{child}");
    }

    /// A worktree of this same repository is the worktree manager's, so the
    /// stage key refuses it rather than recording a link to it.
    #[test]
    fn space_refuses_to_stage_a_linked_worktree() {
        let mut app = test_app(default_bindings());
        app.engine.unstaged_files = vec![ChangedFile {
            kind: ChangedFileKind::LinkedWorktree,
            ..file("inner-wt", "?")
        }];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;
        app.files_index = 0;

        app.handle_key(space()).unwrap();

        assert!(app.changes_tree.pending_ops.is_empty());
        assert!(
            app.status.text().contains("worktree manager"),
            "{}",
            app.status.text()
        );
    }

    /// Staging a repository of its own records a link to it, not its files,
    /// and the status line says exactly that.
    #[test]
    fn staging_a_nested_repository_says_it_records_a_link() {
        let (mut app, worktree) = repo_app();
        let nested = worktree.join("clone");
        std::fs::create_dir_all(&nested).unwrap();
        run_git(&nested, &["init", "-q", "-b", "main"]);
        run_git(&nested, &["config", "user.name", "t"]);
        run_git(&nested, &["config", "user.email", "t@t"]);
        std::fs::write(nested.join("a.txt"), "a\n").unwrap();
        run_git(&nested, &["add", "a.txt"]);
        run_git(&nested, &["commit", "-q", "-m", "a"]);
        load_lists(&mut app, &worktree);
        app.files_index = app
            .changes_rows(RightSection::Unstaged)
            .iter()
            .position(|row| row.file().is_some_and(|f| f.path == "clone"))
            .expect("the nested repository row");

        app.handle_key(space()).unwrap();
        settle(&mut app, idle);

        assert!(
            app.status
                .text()
                .contains("a link to the repository inside it"),
            "{}",
            app.status.text()
        );
        assert!(
            !app.status
                .text()
                .contains("the whole folder is in the staged changes"),
            "{}",
            app.status.text()
        );
    }

    #[test]
    fn space_stages_and_unstages_a_whole_folder_on_a_worker() {
        let (mut app, worktree) = repo_app();

        app.handle_key(space()).unwrap();
        assert_eq!(app.status.tone(), StatusTone::Busy);
        assert!(
            app.status
                .text()
                .contains("Staging \"node_modules/\" (13 files)")
        );
        settle(&mut app, idle);
        assert_eq!(app.status.tone(), StatusTone::Info);
        assert!(app.status.text().contains("Staged \"node_modules/\""));

        load_lists(&mut app, &worktree);
        assert_eq!(
            describe(&app, RightSection::Staged),
            vec!["> node_modules/ 13"]
        );
        assert_eq!(describe(&app, RightSection::Unstaged), vec!["notes.md 1"]);

        app.right_section = RightSection::Staged;
        app.files_index = 0;
        app.handle_key(space()).unwrap();
        settle(&mut app, idle);
        assert!(app.status.text().contains("Unstaged \"node_modules/\""));
        load_lists(&mut app, &worktree);
        assert!(app.engine.staged_files.is_empty());
        assert_eq!(
            describe(&app, RightSection::Unstaged),
            vec!["> node_modules/ 13", "notes.md 1"]
        );
    }

    #[test]
    fn discarding_a_folder_asks_first_then_deletes_it_on_a_worker() {
        let (mut app, worktree) = repo_app();

        app.confirm_discard_selected_file().unwrap();
        let PromptState::ConfirmDiscardFile {
            file_path,
            kind,
            focus,
        } = &app.prompt
        else {
            panic!("the discard asks first");
        };
        assert_eq!(file_path, "node_modules");
        assert!(
            matches!(kind, ChangedFileKind::Directory(contents) if contents.file_count == 13),
            "{kind:?}"
        );
        assert_eq!(*focus, ConfirmFocus::Cancel, "Cancel is focused");
        let screen = render_text(&mut app, 140, 40).join("\n");
        assert!(screen.contains("node_modules/"), "{screen}");
        assert!(screen.contains("13 files"), "{screen}");

        // Cancel keeps everything.
        app.resolve_confirm_discard_file(false);
        assert!(worktree.join("node_modules/top.js").exists());

        app.confirm_discard_selected_file().unwrap();
        app.resolve_confirm_discard_file(true);
        assert_eq!(app.status.tone(), StatusTone::Busy);
        settle(&mut app, idle);
        assert_eq!(app.status.tone(), StatusTone::Info);
        assert!(
            app.status
                .text()
                .contains("Deleted the untracked files in \"node_modules/\" (13 files)"),
            "{}",
            app.status.text()
        );
        assert!(!worktree.join("node_modules").exists());
        load_lists(&mut app, &worktree);
        assert_eq!(describe(&app, RightSection::Unstaged), vec!["notes.md 1"]);
    }

    /// The folder dialog says what the delete keeps as well as what it takes:
    /// ignored files were never counted, and repositories of their own inside
    /// are not entered.
    #[test]
    fn the_folder_delete_dialog_says_what_it_keeps() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmDiscardFile {
            file_path: "vendor".to_string(),
            kind: ChangedFileKind::Directory(dux_core::model::FolderContents {
                file_count: 1_200,
                nested_repositories: 1,
                linked_worktrees: 2,
                ..Default::default()
            }),
            focus: ConfirmFocus::Cancel,
        };

        let screen = render_text(&mut app, 160, 50).join(" ");
        // The body wraps inside the dialog's frame; drop the frame to read it as prose.
        let screen = screen.replace('\u{2502}', " ");
        let screen = screen.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(screen.contains("1,200 files"), "{screen}");
        assert!(
            screen.contains("The 1 nested repository inside it is kept"),
            "{screen}"
        );
        assert!(
            screen.contains("The 2 worktrees of this repository inside it are kept"),
            "{screen}"
        );
        assert!(
            screen.contains("Files the repository ignores inside it are kept"),
            "{screen}"
        );
    }

    #[test]
    fn the_nested_repository_delete_dialog_warns_about_its_history() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmDiscardFile {
            file_path: "vendor/lib".to_string(),
            kind: ChangedFileKind::NestedRepository,
            focus: ConfirmFocus::Cancel,
        };

        let screen = render_text(&mut app, 160, 50).join(" ");
        let screen = screen.replace('\u{2502}', " ");
        let screen = screen.split_whitespace().collect::<Vec<_>>().join(" ");

        assert!(
            screen.contains("including its history and any commits not pushed anywhere else"),
            "{screen}"
        );
    }

    /// A folder holding only repositories of their own has nothing the delete
    /// would take, and a worktree of this repository is the worktree
    /// manager's: neither opens a delete dialog.
    #[test]
    fn the_discard_key_refuses_folders_it_would_not_delete() {
        let mut app = test_app(default_bindings());
        app.engine.unstaged_files = vec![
            ChangedFile {
                kind: ChangedFileKind::Directory(dux_core::model::FolderContents {
                    file_count: 0,
                    nested_repositories: 2,
                    ..Default::default()
                }),
                ..file("vendor", "?")
            },
            ChangedFile {
                kind: ChangedFileKind::LinkedWorktree,
                ..file("inner-wt", "?")
            },
        ];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;

        app.files_index = 0;
        app.confirm_discard_selected_file().unwrap();
        assert!(matches!(app.prompt, PromptState::None));
        assert!(
            app.status
                .text()
                .contains("nothing in \"vendor/\" that a delete would remove"),
            "{}",
            app.status.text()
        );

        app.files_index = 1;
        app.confirm_discard_selected_file().unwrap();
        assert!(matches!(app.prompt, PromptState::None));
        assert!(
            app.status.text().contains("worktree manager"),
            "{}",
            app.status.text()
        );
    }

    #[test]
    fn a_folder_that_became_tracked_is_not_deleted() {
        let (mut app, worktree) = repo_app();
        app.confirm_discard_selected_file().unwrap();
        // An agent commits the folder before the user confirms.
        run_git(&worktree, &["add", "--", "node_modules"]);
        run_git(&worktree, &["commit", "-q", "-m", "vendor"]);

        app.resolve_confirm_discard_file(true);
        settle(&mut app, idle);

        assert_eq!(app.status.tone(), StatusTone::Error);
        assert!(worktree.join("node_modules/top.js").exists());
    }

    #[test]
    fn a_file_inside_an_expanded_folder_stages_on_its_own() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);
        // The top-level file inside the folder.
        app.files_index = 4;
        assert_eq!(
            app.selected_changed_file().map(|f| f.path.as_str()),
            Some("node_modules/top.js")
        );

        app.handle_key(space()).unwrap();

        // Only that file is staged, and it is its own row: the folder is not
        // staged whole, so no folder row may claim it.
        let (staged, unstaged) = git::changed_files(&worktree).unwrap();
        assert_eq!(staged.len(), 1, "{staged:?}");
        assert_eq!(staged[0].path, "node_modules/top.js");
        assert!(!staged[0].is_folder());
        assert_eq!(
            dux_core::model::total_file_count(&unstaged),
            13,
            "the other twelve files and notes.md stay unstaged"
        );
    }

    #[test]
    fn an_expansion_is_forgotten_when_the_folder_leaves_the_listing() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);

        run_git(&worktree, &["add", "--", "node_modules"]);
        load_lists(&mut app, &worktree);

        assert!(
            app.changes_tree.by_session["session-1"].expanded[&ChangesSide::Unstaged].is_empty()
        );
        assert_eq!(describe(&app, RightSection::Unstaged), vec!["notes.md 1"]);
    }

    #[test]
    fn a_folder_whose_count_moved_is_listed_again_quietly() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);
        let settled_message = app.status.text();

        std::fs::write(worktree.join("node_modules/extra.js"), "e\n").unwrap();
        load_lists(&mut app, &worktree);
        assert_eq!(
            app.changes_tree.pending_listings.len(),
            1,
            "the count moved, so the folder is listed again"
        );
        assert!(
            app.changes_tree.pending_listings[0].status_key.is_none(),
            "a refresh of rows already on screen raises no spinner"
        );
        settle(&mut app, idle);

        assert!(
            describe(&app, RightSection::Unstaged)
                .contains(&"  node_modules/extra.js 1".to_string())
        );
        assert_eq!(app.status.text(), settled_message);
    }

    /// An edit to a file already inside the folder keeps its count, so the
    /// folder's fingerprint is what says the rows under it are stale.
    #[test]
    fn a_folder_whose_file_was_edited_is_listed_again_quietly() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);

        std::fs::write(worktree.join("node_modules/top.js"), "top\nmore\nlines\n").unwrap();
        load_lists(&mut app, &worktree);

        assert_eq!(
            app.changes_tree.pending_listings.len(),
            1,
            "the folder's contents changed, so it is listed again"
        );
        settle(&mut app, idle);
        let top = app
            .changes_rows(RightSection::Unstaged)
            .into_iter()
            .find_map(|row| row.file().filter(|f| f.path == "node_modules/top.js"))
            .map(|f| f.additions);
        assert_eq!(top, Some(3), "the edited file's line count is current");
    }

    /// Switching agents empties the lists until the new agent's read lands. A
    /// reconcile against those empty lists would take them for "every folder
    /// is gone" and forget what the user had expanded.
    #[test]
    fn an_agent_switch_does_not_forget_expanded_folders() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);

        app.selected_left = 0;
        app.reload_changed_files();
        app.selected_left = 1;
        app.reload_changed_files();
        assert!(
            app.engine.unstaged_files.is_empty(),
            "the switch emptied the lists"
        );
        app.reconcile_changes_tree();

        assert!(
            app.changes_tree.by_session["session-1"].expanded[&ChangesSide::Unstaged]
                .contains_key("node_modules"),
            "the expansion survives the empty lists"
        );
        load_lists(&mut app, &worktree);
        assert_eq!(
            describe(&app, RightSection::Unstaged)[0],
            "v node_modules/ 13"
        );
    }

    /// A quiet re-list can put rows above the cursor; the cursor stays on the
    /// row it was on rather than on the same row number.
    #[test]
    fn a_quiet_re_list_keeps_the_cursor_on_the_same_row() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);
        app.files_index = app.current_files_len() - 1;
        assert_eq!(
            app.selected_changed_file()
                .map(|f| f.path.clone())
                .as_deref(),
            Some("notes.md")
        );

        std::fs::write(worktree.join("node_modules/aaa.js"), "new\n").unwrap();
        load_lists(&mut app, &worktree);
        settle(&mut app, idle);

        assert_eq!(
            app.selected_changed_file()
                .map(|f| f.path.clone())
                .as_deref(),
            Some("notes.md")
        );
    }

    #[test]
    fn expanded_folders_are_remembered_while_the_selection_moves_away_and_back() {
        let (mut app, worktree) = repo_app();
        app.handle_key(enter()).unwrap();
        settle(&mut app, idle);

        app.selected_left = 0;
        app.reconcile_changes_tree();
        app.selected_left = 1;
        load_lists(&mut app, &worktree);

        assert_eq!(
            describe(&app, RightSection::Unstaged)[0],
            "v node_modules/ 13"
        );
        assert!(app.changes_tree.by_session.contains_key("session-1"));
    }

    #[test]
    fn a_nested_repository_row_neither_expands_nor_opens_a_diff() {
        let mut app = test_app(default_bindings());
        app.engine.unstaged_files = vec![ChangedFile {
            kind: ChangedFileKind::NestedRepository,
            ..file("vendor/lib", "?")
        }];
        app.selected_left = 1;
        app.focus = FocusPane::Files;
        app.right_section = RightSection::Unstaged;
        app.files_index = 0;

        app.handle_key(enter()).unwrap();

        assert!(!matches!(app.center_mode, CenterMode::Diff { .. }));
        assert!(app.changes_tree.pending_listings.is_empty());
        assert!(app.status.text().contains("repository of its own"));
        assert_eq!(
            hint_word(&app),
            None,
            "the key does nothing here, so the hints do not offer it"
        );
    }
}
