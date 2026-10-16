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
    /// A folder check is out; a press waits for it.
    pub(crate) checking: bool,
    /// The folder check's refusal, shown in the form.
    pub(crate) error: Option<String>,
}

/// What the form asks about a review copy for the name now in its field.
pub(crate) enum PrReviewQuestion<'a> {
    /// Nothing to ask.
    None,
    /// A copy will be made; the box agrees to it.
    Needed(&'a [ProseSegment]),
    /// A copy is needed and no name is free; the user must type another name.
    NoFreeName(&'a [ProseSegment]),
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

    /// The question is asked only while the name is the pull request's own
    /// branch: a name typed instead is checked by the server, and its refusal
    /// is the form's error like any other.
    pub(crate) fn question_for(&self, name: &str) -> PrReviewQuestion<'_> {
        if name.trim() != self.head_branch() {
            return PrReviewQuestion::None;
        }
        match &self.review_copy {
            ReviewCopyOutlook::Needed { warning, .. } => PrReviewQuestion::Needed(warning),
            ReviewCopyOutlook::NoFreeName { message, .. } => PrReviewQuestion::NoFreeName(message),
            ReviewCopyOutlook::NotNeeded | ReviewCopyOutlook::Unknown => PrReviewQuestion::None,
        }
    }

    /// The agreement the request carries: exactly the reason and the copy
    /// branch the warning showed, and only while the box is shown and ticked.
    pub(crate) fn agreement_for(&self, name: &str) -> UpfrontReviewCopy {
        match (&self.review_copy, self.question_for(name)) {
            (
                ReviewCopyOutlook::Needed {
                    reason,
                    copy_branch,
                    ..
                },
                PrReviewQuestion::Needed(_),
            ) if self.review_copy_agreed => UpfrontReviewCopy {
                agreed: true,
                reason: Some(reason.clone()),
                branch: Some(copy_branch.clone()),
                by_flag: false,
            },
            _ => UpfrontReviewCopy::default(),
        }
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

/// A folder check's answer, from its worker.
pub(crate) struct PrCloneFolderAnswer {
    /// The folder as it was typed, so an answer for a folder since edited is
    /// dropped.
    pub(crate) destination: String,
    pub(crate) purpose: PrFolderCheck,
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
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        else {
            return;
        };
        form.checking = true;
        form.error = None;
        let raw = destination.text.trim().to_string();
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
                purpose,
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
    /// since edited is dropped. One that differs from what the form shows
    /// switches it, drops the agreement to a copy that no longer applies, and
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
        if destination.text.trim() != answer.destination {
            form.checking = false;
            return;
        }
        form.checking = false;
        let (folder, folder_outlook) = match answer.result {
            Ok(checked) => checked,
            Err(sentence) => {
                form.error = Some(sentence);
                return;
            }
        };
        form.error = None;
        // A free folder is cloned fresh, which the offer already told. A
        // checkout is told from the folder itself.
        let outlook = match (&folder, folder_outlook) {
            (PrFolder::Checkout { .. }, Some(outlook)) => outlook,
            (PrFolder::Checkout { .. }, None) => ReviewCopyOutlook::Unknown,
            (PrFolder::Free, _) => form.offer.review_copy.clone(),
        };
        let changed = folder != form.folder || outlook != form.review_copy;
        let switched_to_add = matches!(folder, PrFolder::Checkout { .. });
        form.folder = folder;
        form.review_copy = outlook;
        if changed {
            form.review_copy_agreed = false;
        }
        match (answer.purpose, changed) {
            (PrFolderCheck::Submit, false) => self.send_pull_request_clone(),
            (PrFolderCheck::Submit, true) => {
                let repository = form.repository();
                self.set_info(if switched_to_add {
                    format!(
                        "That folder already holds a checkout of {repository}, so the form now \
                         adds it as a project instead of cloning. Press {confirm_key} again to \
                         go ahead."
                    )
                } else {
                    format!(
                        "That folder holds nothing of {repository} now, so the form now clones \
                         into it. Press {confirm_key} again to go ahead."
                    )
                });
            }
            (PrFolderCheck::Open, _) => {}
        }
    }

    /// Enter on the form: check what can be checked on screen, then ask the
    /// worker what the folder holds, and go ahead only if it still agrees.
    pub(crate) fn confirm_pull_request_clone(&mut self) {
        let confirm_key = self.bindings.label_for(Action::Confirm);
        let PromptState::CloneProject {
            agent_name,
            focus,
            pull_request: Some(form),
            ..
        } = &mut self.prompt
        else {
            return;
        };
        if form.checking {
            self.set_info(format!(
                "Still checking the folder. Press {confirm_key} again in a moment."
            ));
            return;
        }
        let name = agent_name.text.trim().to_string();
        if name.is_empty() {
            self.set_error("Type a name for the agent, or cancel.");
            return;
        }
        match form.question_for(&name) {
            PrReviewQuestion::Needed(_) if !form.review_copy_agreed => {
                *focus = CloneProjectFocus::ReviewCopyCheckbox;
                self.set_error(
                    "This agent needs a separate review copy of the pull request. Tick Create a \
                     review copy to go ahead, or cancel.",
                );
                return;
            }
            PrReviewQuestion::NoFreeName(message) => {
                let sentence = Prose::from_segments(message.to_vec()).plain();
                self.set_error(sentence);
                return;
            }
            PrReviewQuestion::Needed(_) | PrReviewQuestion::None => {}
        }
        self.spawn_pr_clone_folder_check(PrFolderCheck::Submit);
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

    /// Dispatch the clone or the add. A refusal the engine can give at once
    /// keeps the form open, so it can be acted on without retyping; a start
    /// keeps what was typed as the draft a worker refusal hands back.
    fn send_pull_request_clone(&mut self) {
        let Some(command) = self.pull_request_clone_command() else {
            return;
        };
        let PromptState::CloneProject {
            destination,
            destination_edited,
            start_folder,
            agent_name,
            pull_request: Some(form),
            ..
        } = &self.prompt
        else {
            return;
        };
        let draft = CloneDraft {
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
        };
        match self.engine.apply(command) {
            Ok(reaction) => {
                let refused = matches!(
                    &reaction,
                    EventReaction::Status(update) if update.tone == StatusTone::Error
                );
                if !refused {
                    self.prompt = PromptState::None;
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

    /// A clone or add this form started ended in a project with no agent, for
    /// want of the user's answer on a review copy that only the project could
    /// tell. Open the name prompt for that project and pull request with the
    /// question on it, the way a refused create in an existing project does.
    /// Answers whether it did: an ask that is not this surface's clone, or
    /// whose project is gone, is left to the status line.
    pub(crate) fn reopen_pull_request_after_clone(
        &mut self,
        key: &str,
        question: &ReviewCopyAsk,
    ) -> bool {
        let (Some(project_id), Some(asked)) = (&question.project_id, &question.pull_request) else {
            return false;
        };
        let Some(draft) = self.clone_draft.as_ref().filter(|draft| draft.op_id == key) else {
            return false;
        };
        let Some(facts) = draft
            .pull_request
            .clone()
            .filter(|facts| same_pull_request(&facts.target(), asked))
        else {
            return false;
        };
        let typed_name = draft.agent_name.trim().to_string();
        let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|project| &project.id == project_id)
            .cloned()
        else {
            return false;
        };
        self.clone_draft = None;
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
}
