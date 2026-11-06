//! The changes a user makes to an agent's pull requests (detach, hide, show
//! again, the sort mode and the manual order) and the branch list the Pull
//! requests views read. Attaching is in `mod.rs`, beside the lookup it waits
//! on.
//!
//! None of these needs the GitHub integration or a signed-in `gh`: a pull
//! request must never outlive the ability to remove it. So each one falls back
//! to the agent's rows in the session database, which keeps them while the
//! integration is off, when memory holds none.

use super::{
    Engine, InFlightKey, STANDALONE_ADD_AS_PROJECT_REMEDY, StatusUpdate,
    pr_attach_in_flight_message,
};
use crate::git::GitHubRemote;
use crate::model::{AgentPr, PrKey, PrPrefs, PrState};
use crate::pr_list::{self, Arrange, ArrangeOutcome, MoveTo, PrSortMode};
use crate::status_text::StatusText;
use crate::statusline::QuietSurfaces;
use crate::viewmodel::{PrBranchView, PrBranchesView, PrKeyView};

/// What a refusal about a pull request the agent does not have says, so a
/// route can answer it as a missing resource.
pub const UNKNOWN_PULL_REQUEST_MARKER: &str = "has no pull request";

/// Whether `row` lives in the agent's own repository, `own` being that
/// repository when a worker has read it. Unknown, nothing is the agent's own.
pub(crate) fn in_own_repository(own: Option<&GitHubRemote>, row: &AgentPr) -> bool {
    own.is_some_and(|own| own.holds(&row.host, &row.owner_repo))
}

/// How a sentence names a pull request: `#418` in the agent's own
/// repository, `acme/infra#77` from any other.
pub(crate) fn pr_label(own: Option<&GitHubRemote>, row: &AgentPr) -> String {
    if in_own_repository(own, row) {
        format!("#{}", row.number)
    } else {
        format!("{}#{}", row.owner_repo, row.number)
    }
}

/// "1 pull request", "3 pull requests".
pub(crate) fn pull_request_count(n: usize) -> String {
    if n == 1 {
        "1 pull request".to_string()
    } else {
        format!("{n} pull requests")
    }
}

/// What a standalone agent is refused for every pull-request change and read,
/// in the words [`super::standalone_agent_refusal`] slots it into.
pub(crate) const PR_FEATURE: &str = "attach, detach or track a pull request for";

/// The refusal for moving `label`, one of `agent`'s hidden pull requests.
pub(crate) fn hidden_move_refusal(label: &str, agent: &str) -> String {
    format!("{label} is hidden on \"{agent}\"; show it again before moving it.")
}

/// The refusal for a move to a position `agent`'s `visible` pull requests do
/// not have.
pub(crate) fn position_out_of_range_refusal(agent: &str, visible: usize) -> String {
    format!(
        "\"{agent}\" shows {}; a position is 1 to {visible}.",
        pull_request_count(visible)
    )
}

fn key_label(key: &PrKey) -> String {
    format!("{}#{}", key.owner_repo, key.number)
}

/// An agent's rows and preferences, read for a change.
struct PrChange {
    agent: String,
    rows: Vec<AgentPr>,
    prefs: PrPrefs,
    own: Option<GitHubRemote>,
}

impl PrChange {
    fn index_of(&self, key: &PrKey) -> anyhow::Result<usize> {
        match self.rows.iter().position(|row| row.is(key)) {
            Some(index) => Ok(index),
            None => anyhow::bail!(
                "Agent \"{}\" {UNKNOWN_PULL_REQUEST_MARKER} {}.",
                self.agent,
                key_label(key)
            ),
        }
    }

    fn label(&self, index: usize) -> String {
        pr_label(self.own.as_ref(), &self.rows[index])
    }

    fn visible_len(&self) -> usize {
        pr_list::order(&self.rows, self.prefs.sort_mode)
            .visible
            .len()
    }
}

/// How a move reads: "to the top", "up", "down", "to position 2".
fn move_words(to: MoveTo) -> String {
    match to {
        MoveTo::Top => "to the top".to_string(),
        MoveTo::Up => "up".to_string(),
        MoveTo::Down => "down".to_string(),
        MoveTo::Position(n) => format!("to position {n}"),
    }
}

/// Where a move that changed nothing found the pull request already.
fn edge_words(to: MoveTo) -> String {
    match to {
        MoveTo::Top | MoveTo::Up => "already at the top".to_string(),
        MoveTo::Down => "already at the bottom".to_string(),
        MoveTo::Position(n) => format!("already in position {n}"),
    }
}

/// The tail of a move or drop that switched an automatic order to the
/// user's own, when no order was saved before.
const SWITCHED: &str = " and switched to your own order; choose Automatic under Sort to go back.";

/// The same tail when a saved order was replaced.
const SWITCHED_REPLACING: &str = " and switched to your own order, starting from what was on \
                                  screen; the order you had saved before is replaced.";

/// What an attach says: that it added the pull request, or that the agent
/// already had it. `earlier` is whether the row was there before and, if so,
/// whether it was attached.
pub(crate) fn attach_message(
    agent: &str,
    own: Option<&GitHubRemote>,
    rows: &[AgentPr],
    key: &PrKey,
    earlier: Option<AttachedBefore>,
    mode: PrSortMode,
) -> StatusText {
    let label = rows
        .iter()
        .find(|row| row.is(key))
        .map_or_else(|| key_label(key), |row| pr_label(own, row));
    match earlier {
        Some(AttachedBefore::Attached) => {
            crate::status_text![
                n(label),
                " is already attached to ",
                q(agent),
                "; nothing changed."
            ]
        }
        Some(AttachedBefore::FoundOnly) => crate::status_text![
            n(label),
            " was already found on ",
            q(agent),
            "; it is now attached too, so it stays even if you hide what dux finds."
        ],
        None => {
            let ordered = pr_list::order(rows, mode);
            let count = ordered.visible.len();
            let shown = ordered.visible.first().map(|row| pr_label(own, row));
            let tail = match (mode, shown) {
                _ if count < 2 => crate::status_text!["; the banner shows it."],
                (PrSortMode::Automatic, Some(shown)) => crate::status_text![
                    format!(
                        ". It has {} now; the banner shows ",
                        pull_request_count(count)
                    ),
                    n(shown),
                    " because open ones come first."
                ],
                (PrSortMode::Manual, _) | (_, None) => crate::status_text![format!(
                    ". It has {} now; it went after the ones you placed.",
                    pull_request_count(count)
                )],
            };
            crate::status_text!["Attached ", n(label), " to ", q(agent), tail]
        }
    }
}

/// Whether a pull request being attached was already the agent's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachedBefore {
    /// The user had attached it.
    Attached,
    /// dux had found it, and nobody had attached it.
    FoundOnly,
}

impl Engine {
    /// How the surfaces name one of `session_id`'s pull requests: `#418` in
    /// the agent's own repository, `acme/infra#77` from any other, exactly as
    /// the status messages about it do.
    pub fn pull_request_label(&self, session_id: &str, row: &AgentPr) -> String {
        pr_label(self.pr_own_repos.get(session_id), row)
    }

    /// The agent a pull-request change is for, its rows and its preferences,
    /// or the refusal: an unknown agent, a standalone one, or an attach still
    /// resolving for it (which owns its pull requests until it lands).
    fn begin_pr_change(&self, session_id: &str) -> anyhow::Result<PrChange> {
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            anyhow::bail!("unknown session: {session_id}");
        };
        let agent = session.display_label();
        let _ =
            self.branch_git_workspace(session_id, PR_FEATURE, STANDALONE_ADD_AS_PROJECT_REMEDY)?;
        if self.is_in_flight(&InFlightKey::PrAttach(session_id.to_string())) {
            anyhow::bail!(pr_attach_in_flight_message(&agent));
        }
        let (rows, prefs) = self.pr_change_base(session_id)?;
        let own = self.pr_own_repos.get(session_id).cloned();
        Ok(PrChange {
            agent,
            rows,
            prefs,
            own,
        })
    }

    /// Remember the repository a worker read as `session_id`'s own, moving
    /// the agent's revision when it changed. Reports whether it did.
    pub(crate) fn record_own_pr_repository(
        &mut self,
        session_id: &str,
        remote: GitHubRemote,
    ) -> bool {
        if self.pr_own_repos.get(session_id) == Some(&remote) {
            return false;
        }
        self.pr_own_repos.insert(session_id.to_string(), remote);
        *self.pr_revision.entry(session_id.to_string()).or_default() += 1;
        true
    }

    /// The rows and preferences a change to an agent's pull requests starts
    /// from: memory whenever it holds the agent's rows, because a change
    /// whose save failed lives only there until a restart, and the session
    /// database when it does not (the integration is off, or nothing was
    /// loaded for the agent).
    pub(crate) fn pr_change_base(
        &self,
        session_id: &str,
    ) -> anyhow::Result<(Vec<AgentPr>, PrPrefs)> {
        if let Some(rows) = self.agent_prs.get(session_id) {
            let prefs = self.pr_prefs.get(session_id).cloned().unwrap_or_default();
            return Ok((rows.clone(), prefs));
        }
        let rows = self.session_store.load_agent_prs_of(session_id)?;
        let prefs = self
            .session_store
            .load_pr_prefs()?
            .remove(session_id)
            .unwrap_or_default();
        Ok((rows, prefs))
    }

    /// Store a change and, with the GitHub integration on, mirror it in
    /// memory. Memory takes it even when the write fails, so what the user
    /// just did holds until a restart; the failure is the sentence that says
    /// so. With the integration off memory holds no pull requests, because
    /// nothing shows them, so a change that could not be saved did not
    /// happen, and the failure says only that.
    pub(crate) fn commit_pr_change(
        &mut self,
        session_id: &str,
        agent: &str,
        rows: Vec<AgentPr>,
        prefs: PrPrefs,
    ) -> Result<(), StatusText> {
        let saved = self
            .session_store
            .replace_agent_prs(session_id, &rows, &prefs);
        let mirrored = self.github_integration_enabled;
        if mirrored {
            self.set_pr_prefs(session_id, prefs);
            self.set_agent_prs(session_id, rows);
            self.update_pr_sync_sessions();
            if saved.is_ok() {
                self.pr_unsaved.remove(session_id);
            } else {
                self.pr_unsaved.insert(session_id.to_string());
            }
        }
        saved.map_err(|err| {
            let tail = if mirrored {
                " It holds until dux restarts."
            } else {
                ""
            };
            crate::status_text![
                "Could not save the change to ",
                q(agent),
                format!("'s pull requests: {err:#}.{tail}")
            ]
        })
    }

    fn finish_pr_change(
        &mut self,
        session_id: &str,
        change: PrChange,
        done: StatusUpdate,
    ) -> StatusUpdate {
        match self.commit_pr_change(session_id, &change.agent, change.rows, change.prefs) {
            Ok(()) => done,
            Err(failure) => StatusUpdate::error(failure),
        }
    }

    /// Detach a pull request the user attached. One the sync also found stays
    /// in the list as found; one only attached leaves it. A pull request only
    /// found has nothing to detach and is left as it is, which the answer says.
    ///
    /// # Errors
    ///
    /// Refuses an unknown agent, a standalone one, a pull request the agent
    /// does not have, or a change while an attach resolves; fails when the
    /// session database cannot be read.
    pub fn detach_pull_request(
        &mut self,
        session_id: &str,
        key: &PrKey,
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let index = change.index_of(key)?;
        let label = change.label(index);
        let row = &mut change.rows[index];
        if !row.attached {
            return Ok(StatusUpdate::info(crate::status_text![
                n(label),
                " on ",
                q(change.agent),
                " was found by dux, not attached, so there is nothing to detach; hide it if you \
                 don't want to see it."
            ]));
        }
        let done = if row.found {
            row.attached = false;
            let found_on = if let Some(branch) = row.head_branch.clone() {
                crate::status_text!["on branch ", q(branch)]
            } else {
                crate::status_text!["on one of its branches"]
            };
            crate::status_text![
                "Detached ",
                n(label),
                " from ",
                q(change.agent),
                ". It stays in the list because dux found it ",
                found_on,
                "; hide it if you don't want to see it."
            ]
        } else {
            change.rows.remove(index);
            crate::status_text![
                "Detached ",
                n(label),
                " from ",
                q(change.agent),
                ". Attach it again from Pull requests… if you need it back."
            ]
        };
        Ok(self.finish_pr_change(session_id, change, StatusUpdate::info(done)))
    }

    /// Put a pull request the sync found out of sight. It keeps its place in
    /// the manual order, so showing it again puts it back there.
    ///
    /// # Errors
    ///
    /// Refuses as [`Self::detach_pull_request`] does.
    pub fn hide_pull_request(
        &mut self,
        session_id: &str,
        key: &PrKey,
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let index = change.index_of(key)?;
        let label = change.label(index);
        let row = &mut change.rows[index];
        if !row.found {
            return Ok(StatusUpdate::info(crate::status_text![
                n(label),
                " on ",
                q(change.agent),
                " was attached by you, not found by dux, so it cannot be hidden; detach it if \
                 you don't want it."
            ]));
        }
        if row.hidden {
            return Ok(StatusUpdate::info(crate::status_text![
                n(label),
                " is already hidden on ",
                q(change.agent),
                "; nothing changed."
            ]));
        }
        row.hidden = true;
        let done = crate::status_text![
            "Hid ",
            n(label),
            " on ",
            q(change.agent),
            ". It stays under Hidden in Pull requests… until you show it again."
        ];
        Ok(self.finish_pr_change(session_id, change, StatusUpdate::info(done)))
    }

    /// Show a hidden pull request again, in the place it had.
    ///
    /// # Errors
    ///
    /// Refuses as [`Self::detach_pull_request`] does.
    pub fn show_pull_request(
        &mut self,
        session_id: &str,
        key: &PrKey,
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let index = change.index_of(key)?;
        let label = change.label(index);
        let row = &mut change.rows[index];
        if !row.hidden {
            return Ok(StatusUpdate::info(crate::status_text![
                n(label),
                " is not hidden on ",
                q(change.agent),
                "; nothing changed."
            ]));
        }
        row.hidden = false;
        let done = crate::status_text![
            n(label),
            " is back on ",
            q(change.agent),
            ", in the place it had."
        ];
        Ok(self.finish_pr_change(session_id, change, StatusUpdate::info(done)))
    }

    /// Order an agent's pull requests automatically or in the user's own
    /// order. Neither touches the saved manual order, so choosing Manual again
    /// brings it back.
    ///
    /// # Errors
    ///
    /// Refuses as [`Self::detach_pull_request`] does, less the unknown pull
    /// request.
    pub fn set_pull_request_sort(
        &mut self,
        session_id: &str,
        mode: PrSortMode,
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let agent = change.agent.clone();
        if change.prefs.sort_mode == mode {
            let order = match mode {
                PrSortMode::Automatic => "automatic order",
                PrSortMode::Manual => "your order",
            };
            return Ok(StatusUpdate::info(crate::status_text![
                q(agent),
                format!("'s pull requests are already in {order}; nothing changed.")
            ]));
        }
        change.prefs.sort_mode = mode;
        let done = match mode {
            PrSortMode::Automatic => crate::status_text![
                q(agent),
                "'s pull requests are in automatic order: open, draft, merged, closed, newest \
                 first. Your own order is kept for when you choose Manual."
            ],
            PrSortMode::Manual => crate::status_text![
                q(agent),
                "'s pull requests are in your order. New ones found later go after yours."
            ],
        };
        Ok(self.finish_pr_change(session_id, change, StatusUpdate::info(done)))
    }

    /// Move one visible pull request in the agent's order. In automatic order
    /// the order on screen becomes the user's own, replacing any saved one.
    ///
    /// # Errors
    ///
    /// Refuses as [`Self::detach_pull_request`] does, and refuses a hidden
    /// pull request and a position outside the visible list.
    pub fn move_pull_request(
        &mut self,
        session_id: &str,
        key: &PrKey,
        to: MoveTo,
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let index = change.index_of(key)?;
        let label = change.label(index);
        let agent = change.agent.clone();
        let visible = change.visible_len();
        let mode = change.prefs.sort_mode;
        match pr_list::arrange(
            &mut change.rows,
            mode,
            Arrange::Move {
                key: key.clone(),
                to,
            },
        ) {
            ArrangeOutcome::Moved {
                switched_to_manual,
                replaced_saved,
            } => {
                let moved = crate::status_text![
                    "Moved ",
                    n(label),
                    format!(" {} on ", move_words(to)),
                    q(agent)
                ];
                let done = if switched_to_manual {
                    change.prefs.sort_mode = PrSortMode::Manual;
                    StatusUpdate::info(crate::status_text![
                        moved,
                        if replaced_saved {
                            SWITCHED_REPLACING
                        } else {
                            SWITCHED
                        }
                    ])
                } else {
                    // Quiet on the web: the row moves under the user's own
                    // hand in the open dialog, which is the confirmation.
                    let mut update = StatusUpdate::info(crate::status_text![moved, "."]);
                    update.quiet_on = QuietSurfaces::WEB;
                    update
                };
                Ok(self.finish_pr_change(session_id, change, done))
            }
            ArrangeOutcome::AtEdge => Ok(StatusUpdate::info(crate::status_text![
                n(label),
                format!(" is {} on ", edge_words(to)),
                q(agent),
                "."
            ])),
            ArrangeOutcome::Stale => anyhow::bail!(hidden_move_refusal(&label, &agent)),
            ArrangeOutcome::OutOfRange { .. } => {
                anyhow::bail!(position_out_of_range_refusal(&agent, visible))
            }
        }
    }

    /// Put the agent's visible pull requests in the order a drag left them.
    /// The order must name exactly the visible ones; a list that changed under
    /// the drag is answered with a warning and left as it is.
    ///
    /// # Errors
    ///
    /// Refuses as [`Self::detach_pull_request`] does, less the unknown pull
    /// request.
    pub fn reorder_pull_requests(
        &mut self,
        session_id: &str,
        order: &[PrKey],
    ) -> anyhow::Result<StatusUpdate> {
        let mut change = self.begin_pr_change(session_id)?;
        let agent = change.agent.clone();
        let mode = change.prefs.sort_mode;
        let before: Vec<PrKey> = pr_list::order(&change.rows, mode)
            .visible
            .iter()
            .map(|row| row.key())
            .collect();
        let moved = pr_list::single_moved(&before, order).and_then(|key| {
            let index = change.rows.iter().position(|row| row.is(&key))?;
            let place = order.iter().position(|k| *k == key)?;
            Some((change.label(index), place))
        });
        match pr_list::arrange(
            &mut change.rows,
            mode,
            Arrange::Drop {
                visible_order: order.to_vec(),
            },
        ) {
            ArrangeOutcome::Moved {
                switched_to_manual,
                replaced_saved,
            } => {
                let what = match moved {
                    Some((label, 0)) => crate::status_text!["Moved ", n(label), " to the top on "],
                    Some((label, place)) => crate::status_text![
                        "Moved ",
                        n(label),
                        format!(" to position {} on ", place + 1)
                    ],
                    None => crate::status_text!["Reordered the pull requests on "],
                };
                let done = if switched_to_manual {
                    change.prefs.sort_mode = PrSortMode::Manual;
                    StatusUpdate::info(crate::status_text![
                        what,
                        q(agent),
                        if replaced_saved {
                            SWITCHED_REPLACING
                        } else {
                            SWITCHED
                        }
                    ])
                } else {
                    // Quiet on the web: the drag itself is the confirmation.
                    let mut update = StatusUpdate::info(crate::status_text![what, q(agent), "."]);
                    update.quiet_on = QuietSurfaces::WEB;
                    update
                };
                Ok(self.finish_pr_change(session_id, change, done))
            }
            ArrangeOutcome::AtEdge => Ok(StatusUpdate::info(crate::status_text![
                "The pull requests on ",
                q(agent),
                " are already in that order; nothing changed."
            ])),
            ArrangeOutcome::Stale | ArrangeOutcome::OutOfRange { .. } => {
                Ok(StatusUpdate::warning(crate::status_text![
                    "The pull requests on ",
                    q(agent),
                    " changed while you were dragging; the list has been refreshed, try again."
                ]))
            }
        }
    }

    /// Every branch the agent's worktree has been on, the one checked out now
    /// first and the rest newest first, each with its pull requests, and every
    /// other pull request the agent has, shown or hidden, so none is missing
    /// from the view: attached ones, and found ones whose head is a branch the
    /// list leaves out (the leading branch, found while the worktree was on
    /// it) or not known yet. A pull request is on a branch when it lives in
    /// the agent's own repository and GitHub's head branch is the branch's
    /// name or one of the names it had before a rename; a head of the same
    /// name in another repository is another branch.
    ///
    /// # Errors
    ///
    /// Refuses an unknown agent and a standalone one.
    pub fn pull_request_branches(&self, session_id: &str) -> anyhow::Result<PrBranchesView> {
        let managed =
            self.branch_git_workspace(session_id, PR_FEATURE, STANDALONE_ADD_AS_PROJECT_REMEDY)?;
        let current = managed.branch_name.as_str();
        // The sync never tracks the project's leading branch, so the view
        // does not list it either, unless the worktree is on it right now.
        // Unknown, nothing is excluded by name.
        let leading = self
            .projects
            .iter()
            .find(|p| p.id == managed.project_id)
            .and_then(|p| p.leading_branch.as_deref());
        let rows = self
            .agent_prs
            .get(session_id)
            .map_or(&[][..], Vec::as_slice);
        let ordered = pr_list::order(
            rows,
            self.pr_prefs
                .get(session_id)
                .map(|prefs| prefs.sort_mode)
                .unwrap_or_default(),
        );
        let every: Vec<&AgentPr> = ordered.visible.into_iter().chain(ordered.hidden).collect();
        let mut history: Vec<crate::branch_history::BranchHistoryEntry> = self
            .branch_histories
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        history.retain(|entry| entry.name == current || leading != Some(entry.name.as_str()));
        if !history.iter().any(|entry| entry.name == current) {
            history.push(crate::branch_history::BranchHistoryEntry {
                name: current.to_string(),
                born_here: false,
                identity: None,
                tip: None,
                first_seen_at: None,
                former_names: Vec::new(),
            });
        }
        // The branch checked out now leads, then the rest newest first; a
        // branch not recorded yet is the newest there is.
        history.sort_by(|a, b| {
            (b.name == current).cmp(&(a.name == current)).then_with(|| {
                match (a.first_seen_at, b.first_seen_at) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(x), Some(y)) => y.cmp(&x),
                }
            })
        });
        let own = self.pr_own_repos.get(session_id);
        let on = |entry: &crate::branch_history::BranchHistoryEntry, row: &AgentPr| {
            in_own_repository(own, row)
                && row.head_branch.as_deref().is_some_and(|head| {
                    head == entry.name || entry.former_names.iter().any(|n| n == head)
                })
        };
        let branches = history
            .iter()
            .map(|entry| PrBranchView {
                name: entry.name.clone(),
                former_names: entry.former_names.clone(),
                current: entry.name == current,
                first_seen_at: entry.first_seen_at.map(|at| at.to_rfc3339()),
                prs: every
                    .iter()
                    .filter(|row| on(entry, row))
                    .map(|row| PrKeyView::from(&row.key()))
                    .collect(),
            })
            .collect();
        let on_no_listed_branch = every
            .iter()
            .filter(|row| !history.iter().any(|entry| on(entry, row)))
            .map(|row| PrKeyView::from(&row.key()))
            .collect();
        Ok(PrBranchesView {
            branches,
            on_no_listed_branch,
        })
    }

    /// The open or draft pull request on the branch the agent is on, under
    /// its name now or any it had before a rename, in the agent's own
    /// repository, for the rename dialog's warning. While no pull request
    /// says which branch it is on yet (one carried over from before
    /// pull requests were kept per row, until its first refresh), the open or
    /// draft one shown stands in, unless it is known to live elsewhere. From
    /// what the sync already knows; nothing is asked of GitHub.
    pub fn rename_open_pull_request(&self, session_id: &str) -> Option<u64> {
        let current = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)?
            .workspace
            .as_managed()?
            .branch_name
            .clone();
        let former: Vec<String> = self
            .branch_histories
            .get(session_id)
            .and_then(|history| history.iter().find(|entry| entry.name == current))
            .map(|entry| entry.former_names.clone())
            .unwrap_or_default();
        let rows = self.agent_prs.get(session_id)?;
        let ordered = pr_list::order(
            rows,
            self.pr_prefs
                .get(session_id)
                .map(|prefs| prefs.sort_mode)
                .unwrap_or_default(),
        );
        let own = self.pr_own_repos.get(session_id);
        let open = |row: &AgentPr| matches!(row.state, PrState::Open | PrState::Draft);
        let on_branch = ordered.visible.iter().chain(&ordered.hidden).find(|row| {
            open(row)
                && in_own_repository(own, row)
                && row
                    .head_branch
                    .as_deref()
                    .is_some_and(|head| head == current || former.iter().any(|n| n == head))
        });
        on_branch
            .or_else(|| {
                ordered.visible.first().filter(|row| {
                    open(row)
                        && row.head_branch.is_none()
                        && (own.is_none() || in_own_repository(own, row))
                })
            })
            .map(|row| row.number)
    }
}
