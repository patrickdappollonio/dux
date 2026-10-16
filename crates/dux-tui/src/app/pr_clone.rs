//! The clone form in pull-request mode: what to do when a pull request names a
//! repository no project is a checkout of.
//!
//! The form is the "Clone a repository" form with the address read-only, the
//! name pre-filled, and a body that says which pull request the user is
//! agreeing to. The folder is judged on a worker when the form opens and again
//! on the press, and an answer that differs from the one on screen switches
//! the form and waits for another press, so the user always agrees to what
//! will actually happen. When the agent would need a review copy of the pull
//! request, the form says so up front and holds the button until it is ticked.

use super::components::next_focus;
use super::*;
use crate::git::{agent_name_char_map, is_valid_agent_name};
use dux_core::clone_project::{
    FolderReviewCopyQuery, PrDestination, UpfrontReviewCopy, check_pr_destination,
};
use dux_core::engine::EventReaction;
use dux_core::pr_reference::{
    PullRequestCloneFacts, PullRequestCloneTarget, pr_add_existing_offer_prose,
    pr_clone_offer_incomplete_prose, pr_clone_offer_prose,
};
use dux_core::prose::{Prose, ProseSegment};
use dux_core::status_text::{ReviewCopyAsk, ReviewCopyOutlook};

/// What the folder the form would clone into holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PrFolder {
    /// Missing or empty: the clone goes there.
    Free,
    /// A checkout of the repository, as `path`: it becomes the project.
    Checkout { path: String },
}

/// The pull-request half of the clone form.
#[derive(Clone, Debug)]
pub(crate) struct PullRequestClone {
    /// The pull request as `gh` reported it when the form opened.
    pub(crate) offer: PullRequestCloneFacts,
    /// The reference as the user typed it, parked again when they choose an
    /// existing project instead.
    pub(crate) reference: String,
    /// What could not be checked when no project was found, when something
    /// could not.
    pub(crate) incomplete_summary: Option<String>,
    /// Whether any project exists to choose instead.
    pub(crate) has_projects: bool,
    pub(crate) folder: PrFolder,
    /// Whether the agent will need a review copy, for the folder on screen.
    pub(crate) review_copy: ReviewCopyOutlook,
    pub(crate) review_copy_agreed: bool,
    /// For a checkout, what the last folder check said about the name it was
    /// asked about when that name is not the pull request's branch: a name
    /// spelled like a review copy asks for one only while no local branch of
    /// that name exists, which only the folder can tell.
    pub(crate) checked_review: Option<(String, ReviewCopyOutlook)>,
    /// Counts every change to the name, the box and the folder, so a press
    /// whose folder check is still out can tell it was overtaken.
    pub(crate) edits: u64,
    /// A folder check is out; a press waits for it.
    pub(crate) checking: bool,
    /// That check was asked for by a press, which goes ahead when it agrees.
    pub(crate) submit_pending: bool,
    /// The folder check's refusal, shown in the form.
    pub(crate) error: Option<String>,
}

/// What the form asks about a review copy for the name now in its field.
pub(crate) enum PrReviewQuestion {
    /// Nothing to ask.
    None,
    /// A copy will be made; the box agrees to it.
    Needed(Vec<ProseSegment>),
    /// A copy is needed and no name is free; the user must type another name.
    NoFreeName(Vec<ProseSegment>),
}

impl PullRequestClone {
    pub(crate) fn repository(&self) -> String {
        format!("{}/{}", self.offer.host, self.offer.owner_repo)
    }

    /// The head branch as the name field is compared with it.
    pub(crate) fn head_branch(&self) -> String {
        dux_core::bidi::strip_bidi_controls(&self.offer.head_branch)
    }

    /// The opening sentence, one of the three forms the folder and the check
    /// of the projects call for.
    pub(crate) fn body(&self) -> Prose {
        let repository = self.repository();
        match (&self.folder, &self.incomplete_summary) {
            (PrFolder::Checkout { path }, _) => {
                pr_add_existing_offer_prose(path, &repository, self.offer.number)
            }
            (PrFolder::Free, Some(summary)) => {
                pr_clone_offer_incomplete_prose(&repository, self.offer.number, summary)
            }
            (PrFolder::Free, None) => pr_clone_offer_prose(&repository, self.offer.number),
        }
    }

    /// Whether the agent named `name` will need a review copy, for the folder
    /// on screen. The pull request's own branch is told by the offer or the
    /// folder; a name spelled like a review copy of it is one whatever the
    /// branch's state, except in a checkout that already has a branch of that
    /// name, which the last folder check says; any other name needs none.
    pub(crate) fn outlook_for(&self, name: &str) -> ReviewCopyOutlook {
        let name = name.trim();
        let head = self.head_branch();
        if name == head {
            return self.review_copy.clone();
        }
        if let Some((checked, outlook)) = &self.checked_review
            && checked == name
        {
            return outlook.clone();
        }
        dux_core::agent_job::typed_copy_review_outlook(&head, name, self.offer.number)
            .unwrap_or(ReviewCopyOutlook::NotNeeded)
    }

    /// What the form asks about a review copy for the name now in its field.
    pub(crate) fn question_for(&self, name: &str) -> PrReviewQuestion {
        match self.outlook_for(name) {
            ReviewCopyOutlook::Needed { warning, .. } => PrReviewQuestion::Needed(warning),
            ReviewCopyOutlook::NoFreeName { message, .. } => PrReviewQuestion::NoFreeName(message),
            ReviewCopyOutlook::NotNeeded | ReviewCopyOutlook::Unknown => PrReviewQuestion::None,
        }
    }

    /// The agreement the request carries: exactly the reason and the copy
    /// branch the warning showed, and only while the box is shown and ticked.
    pub(crate) fn agreement_for(&self, name: &str) -> UpfrontReviewCopy {
        match self.outlook_for(name) {
            ReviewCopyOutlook::Needed {
                reason,
                copy_branch,
                ..
            } if self.review_copy_agreed => UpfrontReviewCopy {
                agreed: true,
                reason: Some(reason),
                branch: Some(copy_branch),
                by_flag: false,
            },
            _ => UpfrontReviewCopy::default(),
        }
    }

    /// The name changed: the answer given to the old question is not one to the
    /// new, and a press overtaken by this must not go ahead.
    pub(crate) fn name_edited(&mut self) {
        self.review_copy_agreed = false;
        self.edits += 1;
    }

    /// The folder changed, so everything told about the old one is stale: the
    /// form goes back to what the offer said, unticked, and the next press
    /// checks the new folder instead of repeating the old answer.
    pub(crate) fn destination_edited(&mut self) {
        self.folder = PrFolder::Free;
        self.review_copy = self.offer.review_copy.clone();
        self.checked_review = None;
        self.review_copy_agreed = false;
        self.error = None;
        self.edits += 1;
    }

    /// The Tab ring for the name now in the field: each stop and whether it is
    /// present.
    pub(crate) fn focus_ring(&self, name: &str) -> [(CloneProjectFocus, bool); 4] {
        [
            (CloneProjectFocus::Destination, true),
            (CloneProjectFocus::AgentName, true),
            (
                CloneProjectFocus::ReviewCopyCheckbox,
                matches!(self.question_for(name), PrReviewQuestion::Needed(_)),
            ),
            (CloneProjectFocus::ChooseExisting, self.has_projects),
        ]
    }
}

/// Why a folder check ran, which decides what its answer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrFolderCheck {
    /// The form just opened: the answer only sets the state on screen.
    Open,
    /// The user pressed the button: an unchanged answer goes ahead.
    Submit,
}

/// What changed in the form, for [`App::note_pr_clone_edit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrEdit {
    Name,
    Folder,
    Box,
}

/// A clone or add this surface started for a pull request, kept so a question
/// only the server can ask about it (it comes back under the key of whatever
/// operation ends it) can put the user back where they were.
#[derive(Clone, Debug)]
pub(crate) struct PrCloneAwaiting {
    form: PullRequestClone,
    draft: CloneDraft,
    since: Instant,
}

/// How long a started clone waits for such a question. A clone and the agent
/// create after it take minutes at the outside.
const PR_CLONE_AWAITING_BOUND: Duration = Duration::from_secs(60 * 60);

/// A folder check's answer, from its worker.
pub(crate) struct PrCloneFolderAnswer {
    /// The folder as it was typed, so an answer for a folder since edited is
    /// dropped.
    pub(crate) destination: String,
    /// The name the check was asked about, which decides the question for a
    /// name spelled like a review copy.
    pub(crate) agent_name: String,
    /// The form's edit count when the check started.
    pub(crate) edits: u64,
    pub(crate) result: Result<(PrFolder, Option<ReviewCopyOutlook>), String>,
}

/// Whether two pull requests are the same one, the repository compared
/// case-insensitively as the project match compares it.
pub(crate) fn same_pull_request(a: &PullRequestCloneTarget, b: &PullRequestCloneTarget) -> bool {
    a.number == b.number
        && a.host.eq_ignore_ascii_case(&b.host)
        && a.owner_repo.eq_ignore_ascii_case(&b.owner_repo)
}

impl App {
    /// Open the clone form for a pull request no project has the repository
    /// of. Restores what an earlier, refused attempt for the same pull request
    /// was given; a draft for another pull request, or a plain clone's, stays
    /// where it is for the opener it belongs to.
    pub(crate) fn open_pull_request_clone_form(
        &mut self,
        reference: String,
        incomplete_summary: Option<String>,
        offer: PullRequestCloneFacts,
    ) {
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        let target = offer.target();
        let restored = self
            .clone_draft
            .as_ref()
            .filter(|draft| {
                draft.restorable
                    && draft
                        .pull_request
                        .as_ref()
                        .is_some_and(|facts| same_pull_request(&facts.target(), &target))
            })
            .cloned();
        let mut agent_name = TextInput::new().with_char_map(agent_name_char_map);
        let (destination, destination_edited, start_folder) = match restored {
            Some(draft) => {
                agent_name.set_text(draft.agent_name);
                (
                    draft.destination,
                    draft.destination_edited,
                    draft.start_folder,
                )
            }
            None => {
                // Never a made-up name: the pull request's own branch, when it
                // is a name an agent can have, else nothing until the user
                // types one.
                let head = dux_core::bidi::strip_bidi_controls(&offer.head_branch);
                if is_valid_agent_name(&head) {
                    agent_name.set_text(head);
                }
                let start_folder =
                    dux_core::project_browser::resolve_start_dir(&self.engine.config);
                (
                    super::sessions::clone_destination_for(&start_folder, &offer.address),
                    false,
                    start_folder,
                )
            }
        };
        let form = PullRequestClone {
            review_copy: offer.review_copy.clone(),
            offer,
            reference,
            incomplete_summary,
            has_projects: !self.engine.projects.is_empty(),
            folder: PrFolder::Free,
            review_copy_agreed: false,
            checked_review: None,
            edits: 0,
            submit_pending: false,
            checking: false,
            error: None,
        };
        self.prompt = PromptState::CloneProject {
            address: TextInput::with_text(form.offer.address.clone()),
            destination: TextInput::with_text(destination),
            destination_edited,
            start_folder,
            agent_name,
            randomize_name: false,
            randomized_name: None,
            focus: CloneProjectFocus::Destination,
            pull_request: Some(Box::new(form)),
        };
        self.spawn_pr_clone_folder_check(PrFolderCheck::Open);
    }

    /// Ask a worker what the destination field holds. The answer lands in
    /// [`Self::drain_pr_clone_folder_check`].
    pub(crate) fn spawn_pr_clone_folder_check(&mut self, purpose: PrFolderCheck) {
        let PromptState::CloneProject {
            destination,
            agent_name,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        else {
            return;
        };
        form.checking = true;
        form.submit_pending = purpose == PrFolderCheck::Submit;
        form.error = None;
        let raw = destination.text.trim().to_string();
        let name = agent_name.text.trim().to_string();
        let edits = form.edits;
        let offer = form.offer.clone();
        let policy = self.engine.github_host_policy();
        let projects: Vec<String> = self
            .engine
            .projects
            .iter()
            .map(|project| project.path.clone())
            .collect();
        let worktrees_root = self.engine.paths.worktrees_root.clone();
        let (tx, rx) = mpsc::channel();
        // A check still out is for a folder the user has since left behind.
        self.pr_clone_check_rx = Some(rx);
        thread::spawn(move || {
            let project_refs: Vec<&str> = projects.iter().map(String::as_str).collect();
            let result = check_pr_destination(
                &raw,
                &offer.target(),
                &policy,
                &project_refs,
                Some(FolderReviewCopyQuery {
                    worktrees_root: &worktrees_root,
                    number: offer.number,
                    head_branch: &offer.head_branch,
                    head_commit: offer.head_oid.as_deref(),
                    agent_name: Some(&name)
                        .filter(|name| !name.is_empty())
                        .map(String::as_str),
                }),
            )
            .map(|check| {
                let folder = match check.state {
                    PrDestination::Free => PrFolder::Free,
                    PrDestination::Checkout => PrFolder::Checkout {
                        path: check.path.display().to_string(),
                    },
                };
                (folder, check.review_copy)
            })
            .map_err(|refused| refused.to_string());
            let _ = tx.send(PrCloneFolderAnswer {
                destination: raw,
                agent_name: name,
                edits,
                result,
            });
        });
    }

    /// Fold a landed folder check into the form that asked for it.
    pub(crate) fn drain_pr_clone_folder_check(&mut self) {
        let Some(rx) = self.pr_clone_check_rx.as_ref() else {
            return;
        };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pr_clone_check_rx = None;
                let confirm_key = self.bindings.label_for(Action::Confirm);
                if let PromptState::CloneProject {
                    pull_request: Some(form),
                    ..
                } = &mut self.prompt
                {
                    form.checking = false;
                    form.error = Some(format!(
                        "The folder check stopped unexpectedly. Press {confirm_key} to check the \
                         folder again."
                    ));
                }
                self.mark_frame_dirty();
                return;
            }
        };
        self.pr_clone_check_rx = None;
        self.apply_pr_clone_folder_answer(answer);
        self.mark_frame_dirty();
    }

    /// What a folder check's answer does to the form. An answer for a folder
    /// since edited is dropped. One that differs from what the form shows (the
    /// folder's state, or whether it needs a review copy) switches it, drops
    /// the agreement to a copy that no longer applies, says what changed, and
    /// waits for another press: the user agrees to what will actually happen.
    pub(crate) fn apply_pr_clone_folder_answer(&mut self, answer: PrCloneFolderAnswer) {
        let confirm_key = self.bindings.label_for(Action::Confirm);
        let PromptState::CloneProject {
            destination,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        else {
            return;
        };
        form.checking = false;
        let was_pressed = std::mem::take(&mut form.submit_pending);
        if destination.text.trim() != answer.destination {
            return;
        }
        let (folder, folder_outlook) = match answer.result {
            Ok(checked) => checked,
            Err(sentence) => {
                form.error = Some(sentence);
                return;
            }
        };
        form.error = None;
        let name = answer.agent_name;
        let folder_before = form.folder.clone();
        let outlook_before = form.outlook_for(&name);
        // A free folder is cloned fresh, which the offer already told. A
        // checkout is told from the folder itself.
        match &folder {
            PrFolder::Free => {
                form.review_copy = form.offer.review_copy.clone();
                form.checked_review = None;
            }
            PrFolder::Checkout { .. } => {
                let outlook = folder_outlook.unwrap_or(ReviewCopyOutlook::Unknown);
                if name == form.head_branch() {
                    form.review_copy = outlook;
                    form.checked_review = None;
                } else {
                    form.checked_review = Some((name.clone(), outlook));
                }
            }
        }
        form.folder = folder;
        let outlook_after = form.outlook_for(&name);
        let folder_changed = folder_before != form.folder;
        let outlook_changed = outlook_before != outlook_after;
        if folder_changed || outlook_changed {
            form.review_copy_agreed = false;
        }
        let asks_now = match &outlook_after {
            ReviewCopyOutlook::Needed { .. } => Some(
                "Tick Create a review copy, or type another name, then press the confirm key \
                 again.",
            ),
            ReviewCopyOutlook::NoFreeName { .. } => {
                Some("Every name a review copy could take is taken. Type another name.")
            }
            ReviewCopyOutlook::NotNeeded | ReviewCopyOutlook::Unknown => None,
        };
        let repository = form.repository();
        let overtaken = answer.edits != form.edits;
        let go_ahead = was_pressed && !overtaken && !folder_changed && !outlook_changed;
        let said = if !was_pressed || go_ahead {
            None
        } else if overtaken {
            Some(format!(
                "The form changed while the folder was being checked, so nothing was sent. \
                 Press {confirm_key} again."
            ))
        } else if folder_changed {
            let lead = if matches!(form.folder, PrFolder::Checkout { .. }) {
                format!(
                    "That folder already holds a checkout of {repository}, so the form now adds \
                     it as a project instead of cloning."
                )
            } else {
                format!(
                    "That folder holds nothing of {repository} now, so the form now clones into \
                     it."
                )
            };
            Some(match asks_now {
                Some(ask) => format!("{lead} {}", ask.replace("the confirm key", &confirm_key)),
                None => format!("{lead} Press {confirm_key} again to go ahead."),
            })
        } else {
            Some(match asks_now {
                Some(ask) => format!(
                    "The review copy answer for that folder changed. {}",
                    ask.replace("the confirm key", &confirm_key)
                ),
                None => format!(
                    "That folder no longer needs a review copy. Press {confirm_key} again to go \
                     ahead."
                ),
            })
        };
        if let Some(said) = said {
            self.set_info(said);
        }
        self.settle_pr_clone_focus();
        if go_ahead {
            self.send_pull_request_clone();
        }
    }

    /// A box that is no longer shown cannot keep focus: it moves on to the
    /// next stop that is.
    pub(crate) fn settle_pr_clone_focus(&mut self) {
        if let PromptState::CloneProject {
            focus,
            agent_name,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
            && *focus == CloneProjectFocus::ReviewCopyCheckbox
            && !matches!(
                form.question_for(&agent_name.text),
                PrReviewQuestion::Needed(_)
            )
        {
            *focus = next_focus(&form.focus_ring(&agent_name.text), *focus, true);
        }
    }

    /// The name, the folder or the box changed. The form forgets what the old
    /// value was told, and a press whose folder check is still out is
    /// overtaken: it says so in the form, so the user is never left wondering
    /// why nothing happened.
    pub(crate) fn note_pr_clone_edit(&mut self, edit: PrEdit) {
        let confirm_key = self.bindings.label_for(Action::Confirm);
        if let PromptState::CloneProject {
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        {
            match edit {
                PrEdit::Name => form.name_edited(),
                PrEdit::Folder => form.destination_edited(),
                PrEdit::Box => form.edits += 1,
            }
            if form.submit_pending {
                form.error = Some(match edit {
                    PrEdit::Folder => format!(
                        "The folder changed, so nothing will be sent. Press {confirm_key} to \
                         check it again."
                    ),
                    PrEdit::Name | PrEdit::Box => format!(
                        "That changed while the folder was being checked, so nothing will be \
                         sent. Press {confirm_key} again."
                    ),
                });
            }
        }
        self.settle_pr_clone_focus();
    }

    /// Enter on the form: check what can be checked on screen, then ask the
    /// worker what the folder holds, and go ahead only if it still agrees.
    pub(crate) fn confirm_pull_request_clone(&mut self) {
        let confirm_key = self.bindings.label_for(Action::Confirm);
        let PromptState::CloneProject {
            pull_request: Some(form),
            ..
        } = &self.prompt
        else {
            return;
        };
        if form.checking {
            self.set_info(format!(
                "Still checking the folder. Press {confirm_key} again in a moment."
            ));
            return;
        }
        if self.pr_clone_blocked() {
            return;
        }
        self.spawn_pr_clone_folder_check(PrFolderCheck::Submit);
    }

    /// Whether what is in the form now may not be sent, saying why when so: an
    /// empty or unusable name, a review copy nobody agreed to, a copy with no
    /// free name. Asked when the press arrives and again right before the
    /// request leaves, on the form as it is then.
    fn pr_clone_blocked(&mut self) -> bool {
        let PromptState::CloneProject {
            agent_name,
            focus,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        else {
            return true;
        };
        let name = agent_name.text.trim().to_string();
        if name.is_empty() {
            self.set_error("Type a name for the agent, or cancel.");
            return true;
        }
        if !is_valid_agent_name(&name) {
            self.set_error(
                "Agent name may only contain letters, digits, dashes, underscores, or slashes. \
                 It cannot start with \"-\" or \"/\", end with \"/\", or contain \"//\".",
            );
            return true;
        }
        match form.question_for(&name) {
            PrReviewQuestion::Needed(_) if !form.review_copy_agreed => {
                *focus = CloneProjectFocus::ReviewCopyCheckbox;
                self.set_error(
                    "This agent needs a separate review copy of the pull request. Tick Create a \
                     review copy to go ahead, or cancel.",
                );
                true
            }
            PrReviewQuestion::NoFreeName(message) => {
                self.set_error(Prose::from_segments(message).plain());
                true
            }
            PrReviewQuestion::Needed(_) | PrReviewQuestion::None => false,
        }
    }

    /// The request the form would send for the fields as they stand: the
    /// target and no address, because the server builds the address again.
    pub(crate) fn pull_request_clone_command(&self) -> Option<Command> {
        let PromptState::CloneProject {
            destination,
            agent_name,
            pull_request: Some(form),
            ..
        } = &self.prompt
        else {
            return None;
        };
        let name = agent_name.text.trim();
        Some(Command::CloneProject {
            // The server builds the address again from current state.
            url: String::new(),
            path: destination.text.trim().to_string(),
            agent_name: Some(name.to_string()),
            random_name: false,
            pull_request: Some(form.offer.target()),
            add_existing: matches!(form.folder, PrFolder::Checkout { .. }),
            review_copy: form.agreement_for(name),
        })
    }

    /// Dispatch the clone or the add, from the form as it is now. A refusal the
    /// engine can give at once keeps the form open, so it can be acted on
    /// without retyping; a start keeps what was typed as the draft a worker
    /// refusal hands back, and remembers the form for a question only the
    /// server can ask.
    fn send_pull_request_clone(&mut self) {
        if self.pr_clone_blocked() {
            return;
        }
        let (Some(command), Some(awaiting)) =
            (self.pull_request_clone_command(), self.pr_clone_snapshot())
        else {
            return;
        };
        let draft = awaiting.draft.clone();
        match self.engine.apply(command) {
            Ok(reaction) => {
                let refused = matches!(
                    &reaction,
                    EventReaction::Status(update) if update.tone == StatusTone::Error
                );
                if !refused {
                    self.prompt = PromptState::None;
                    self.pr_clone_awaiting = Some(awaiting);
                    self.clone_draft = match &reaction {
                        EventReaction::Status(update) => update
                            .key
                            .clone()
                            .map(|op_id| CloneDraft { op_id, ..draft }),
                        _ => None,
                    };
                }
                self.apply_reaction(reaction);
            }
            Err(err) => self.set_error(format!("Could not start the clone: {err:#}")),
        }
    }

    /// The form as it stands, kept for the draft a worker refusal hands back
    /// and for a question only the server can ask about the request.
    pub(crate) fn pr_clone_snapshot(&self) -> Option<PrCloneAwaiting> {
        let PromptState::CloneProject {
            destination,
            destination_edited,
            start_folder,
            agent_name,
            pull_request: Some(form),
            ..
        } = &self.prompt
        else {
            return None;
        };
        Some(PrCloneAwaiting {
            form: (**form).clone(),
            draft: CloneDraft {
                op_id: String::new(),
                restorable: false,
                address: form.offer.address.clone(),
                destination: destination.text.clone(),
                destination_edited: *destination_edited,
                start_folder: start_folder.clone(),
                agent_name: agent_name.text.clone(),
                randomize_name: false,
                randomized_name: None,
                pull_request: Some(form.offer.clone()),
            },
            since: Instant::now(),
        })
    }

    /// Flip the form's review copy box. Focus follows, so a click is visibly
    /// acted on.
    pub(crate) fn toggle_pr_clone_review_copy(&mut self) {
        if let PromptState::CloneProject {
            focus,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        {
            *focus = CloneProjectFocus::ReviewCopyCheckbox;
            form.review_copy_agreed = !form.review_copy_agreed;
        }
        self.note_pr_clone_edit(PrEdit::Box);
    }

    /// Leave the form for the project picker, keeping the reference typed.
    pub(crate) fn choose_existing_project_from_clone_form(&mut self) {
        let PromptState::CloneProject {
            pull_request: Some(form),
            ..
        } = &self.prompt
        else {
            return;
        };
        self.pending_pr_reference = Some(form.reference.clone());
        self.pr_clone_check_rx = None;
        if let Err(err) =
            self.open_project_chooser_over(ProjectChooserIntent::FromPrReference, None)
        {
            self.set_error(format!("{err:#}"));
        }
    }

    /// The remembered clone this surface started, when `asked` is its pull
    /// request. The server's final for it comes under the key of whatever
    /// operation ends it, not the clone's, so the pull request is what ties
    /// the two together. Kept until a question for it arrives or the bound
    /// passes.
    fn awaiting_for(&mut self, asked: &PullRequestCloneTarget) -> Option<PrCloneAwaiting> {
        let fresh = self
            .pr_clone_awaiting
            .as_ref()
            .is_some_and(|held| held.since.elapsed() < PR_CLONE_AWAITING_BOUND);
        if !fresh {
            self.pr_clone_awaiting = None;
            return None;
        }
        if self
            .pr_clone_awaiting
            .as_ref()
            .is_some_and(|held| same_pull_request(&held.form.offer.target(), asked))
        {
            return self.pr_clone_awaiting.take();
        }
        None
    }

    /// A question the server asked about a clone or add this surface started.
    ///
    /// With a project in it, the project exists and has no agent for want of
    /// the user's answer: the name prompt opens for that project and pull
    /// request with the question on it, the way a refused create in an
    /// existing project does. Without one, the server refused before cloning
    /// or adding anything: the pull request form comes back with the same
    /// values and the new question, unticked. Answers whether it did either;
    /// a question that is not this surface's clone is left to the status line.
    pub(crate) fn reopen_pull_request_after_clone(&mut self, question: &ReviewCopyAsk) -> bool {
        let Some(asked) = &question.pull_request else {
            return false;
        };
        let project = match &question.project_id {
            Some(project_id) => {
                let Some(project) = self
                    .engine
                    .projects
                    .iter()
                    .find(|project| &project.id == project_id)
                    .cloned()
                else {
                    return false;
                };
                Some(project)
            }
            None => None,
        };
        let Some(held) = self.awaiting_for(asked) else {
            return false;
        };
        let facts = held.form.offer.clone();
        let typed_name = held.draft.agent_name.trim().to_string();
        let Some(project) = project else {
            self.reopen_pull_request_form_with(held, question);
            return true;
        };
        let request = CreateAgentRequest::PullRequest {
            project,
            host: facts.host,
            owner_repo: facts.owner_repo,
            number: facts.number,
            title: facts.title,
            state: facts.state,
            head_branch: facts.head_branch.clone(),
            custom_name: Some(if typed_name.is_empty() {
                dux_core::bidi::strip_bidi_controls(&facts.head_branch)
            } else {
                typed_name
            }),
            use_existing_branch: false,
            review_copy: Default::default(),
            ask_id: None,
            agents: Vec::new(),
            cloned: None,
        };
        self.reopen_name_prompt_for_review_copy(
            request,
            question.warned.clone(),
            question.warning.clone(),
        );
        true
    }

    /// Put the form back as it was sent, with the server's question in place of
    /// whatever it was told before, unticked.
    fn reopen_pull_request_form_with(&mut self, held: PrCloneAwaiting, question: &ReviewCopyAsk) {
        let PrCloneAwaiting {
            mut form, draft, ..
        } = held;
        if let (Some(reason), Some(copy_branch)) = (&question.reason, &question.copy_branch) {
            let outlook = ReviewCopyOutlook::Needed {
                reason: reason.clone(),
                branch: form.offer.head_branch.clone(),
                copy_branch: copy_branch.clone(),
                warning: question.warning.clone(),
            };
            if draft.agent_name.trim() == form.head_branch() {
                form.review_copy = outlook;
            } else {
                form.checked_review = Some((draft.agent_name.trim().to_string(), outlook));
            }
        }
        form.review_copy_agreed = false;
        form.checking = false;
        form.submit_pending = false;
        form.error = None;
        let mut agent_name = TextInput::new().with_char_map(agent_name_char_map);
        agent_name.set_text(draft.agent_name);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::CloneProject {
            address: TextInput::with_text(form.offer.address.clone()),
            destination: TextInput::with_text(draft.destination),
            destination_edited: draft.destination_edited,
            start_folder: draft.start_folder,
            agent_name,
            randomize_name: false,
            randomized_name: None,
            focus: CloneProjectFocus::ReviewCopyCheckbox,
            pull_request: Some(Box::new(form)),
        };
    }
}
