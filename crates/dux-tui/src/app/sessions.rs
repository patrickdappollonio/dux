use super::*;
use crate::browser;
use crate::editor;
use dux_core::engine::{Command, EventReaction, FinishDeleteSessionOutcome, WorktreeRemoval};

impl App {
    pub(crate) fn open_project_browser(&mut self) -> Result<()> {
        self.open_folder_browser(BrowsePurpose::AddProject)
    }

    /// Palette command (`new-standalone-agent`): pick a folder you already
    /// have and run a provider in it.
    ///
    /// It reuses the same folder browser as adding a project, with the purpose
    /// switched: the listing already includes plain directories, and the only
    /// difference is that picking one goes nowhere near the add-project
    /// validator, which rejects exactly the plain folder that is the ordinary
    /// case here.
    pub(crate) fn open_standalone_agent_browser(&mut self) -> Result<()> {
        self.open_folder_browser(BrowsePurpose::StandaloneAgent)
    }

    /// Create a standalone agent in the folder the browser is currently in.
    ///
    /// Deliberately does NOT go through `validate_project_add_path`: that
    /// validator rejects a folder which is not a repository root, and a plain
    /// folder is the ordinary case here. Nothing is initialized in the user's
    /// directory either; dux just runs the provider in it.
    ///
    /// The refusals (a relative path, a folder that already hosts a standalone
    /// agent) come back from the shared wire arm, so the TUI and the web say
    /// the same thing for the same reason.
    pub(crate) fn create_standalone_agent_in(&mut self, path: String) {
        self.prompt = PromptState::None;
        // No name typed: the title comes from the folder's own name, and
        // renaming afterwards is an ordinary rename like any agent's. The
        // provider is the global default, retargetable afterwards.
        match self.engine.plan_standalone_agent(&path, "", None) {
            Ok((request, busy_message)) => {
                if let Err(err) = self.dispatch_create_agent_request(request, busy_message) {
                    self.set_error(format!("Could not create the standalone agent: {err:#}"));
                }
            }
            Err(err) => self.set_error(err.to_string()),
        }
    }

    fn open_folder_browser(&mut self, purpose: BrowsePurpose) -> Result<()> {
        let start_dir = dux_core::project_browser::resolve_start_dir(&self.engine.config);
        self.prompt = PromptState::BrowseProjects {
            purpose,
            current_dir: start_dir.clone(),
            entries: Vec::new(),
            loading: true,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        self.engine.spawn_browser_entries(&start_dir);
        {
            let open = self.bindings.label_for(Action::OpenEntry);
            let add = self.bindings.label_for(Action::AddCurrentDir);
            let search = self.bindings.label_for(Action::SearchToggle);
            let goto = self.bindings.label_for(Action::GoToPath);
            let what = match purpose {
                BrowsePurpose::AddProject => "adds current dir",
                BrowsePurpose::StandaloneAgent => "runs an agent in current dir",
            };
            self.set_info(format!(
                "Folder browser: {open} opens folders, {add} {what}, {search} to search, {goto} to go to a path.",
            ));
        }
        Ok(())
    }

    pub(crate) fn add_project(&mut self, raw_path: String, name: String) -> Result<()> {
        let path = match self.engine.validate_project_add_path(&raw_path) {
            Ok(path) => path,
            Err(message) => {
                self.set_error(message);
                return Ok(());
            }
        };
        logger::info(&format!("attempting to add project {}", path.display()));

        // Run the git probes, then let the CORE-owned `add_project_plan` decide
        // the action + warning (the single-source decision the web's inspect
        // endpoint also consumes, pinned by the shared vector matrix). The TUI
        // renders its own dialog copy from the returned typed codes. `validate_`
        // `project_add_path` above already rejected blocked/non-repo paths, so
        // only the unborn-commit and branch-warning rungs are reachable here.
        let branch = git::current_branch_opt(&path)?.unwrap_or_default();
        let branch_warning = (!branch.is_empty())
            .then(|| git::branch_warning_kind(&path, &branch))
            .flatten();
        let inspection = dux_core::add_project_plan::AddProjectInspection {
            path_kind: git::repo_path_kind(&path),
            current_branch: (!branch.is_empty()).then(|| branch.clone()),
            branch_warning,
            // A CONFIRMED unborn HEAD needs the initial-commit path; an
            // indeterminate git result fails OPEN (treated as having commits) so
            // a transient failure never hijacks a normal add with the commit
            // dialog. `repo_has_commits` returns true unless the repo is
            // definitively unborn.
            has_commits: git::repo_commit_state(&path) != git::CommitState::Unborn,
        };
        let plan = dux_core::add_project_plan::add_project_plan(&inspection);

        use dux_core::add_project_plan::{AddProjectAction, AddProjectWarning};
        if matches!(plan.action, AddProjectAction::NeedsInitialCommit) {
            self.prompt = PromptState::ConfirmCreateInitialCommit {
                path: path.to_string_lossy().to_string(),
                name,
                focus: ConfirmFocus::Cancel,
            };
            return Ok(());
        }

        let leading_branch =
            leading_branch_for_project(&path, (!branch.is_empty()).then_some(branch.as_str()));

        // A non-default-branch warning maps back to the TUI's existing
        // `BranchWarningKind` for the ConfirmNonDefaultBranch dialog. `None`
        // (default branch or detached HEAD) falls through to the direct add.
        let warning_kind = match &plan.warning {
            AddProjectWarning::NotOnDefaultBranch { default_branch } => {
                Some(BranchWarningKind::Known {
                    default_branch: default_branch.clone(),
                })
            }
            AddProjectWarning::NotOnDefaultBranchUnknown => Some(BranchWarningKind::Heuristic),
            AddProjectWarning::None => None,
        };
        if let Some(kind) = warning_kind {
            self.prompt = PromptState::ConfirmNonDefaultBranch {
                action: NonDefaultBranchAction::AddProject {
                    path: path.to_string_lossy().to_string(),
                    name,
                    leading_branch,
                },
                current_branch: branch,
                kind,
                focus: ConfirmNonDefaultBranchFocus::Cancel,
                // Only the known-default warning offers the checkout (the
                // heuristic path shows no checkbox); `can_checkout_default` is
                // the core rule.
                checkout_default: plan.can_checkout_default,
            };
            return Ok(());
        }

        let path_str = path.to_string_lossy().to_string();
        self.finish_add_project(path_str, name, branch, leading_branch)
    }

    /// Saves the project to SQLite and config.toml INLINE (no background worker):
    /// the engine writes both synchronously, rolling back the SQLite row if the
    /// config write fails, and the project is in the runtime list with a final
    /// status (success or rollback error) by the time this returns.
    /// Called directly when no branch warning is needed, or after the user
    /// confirms "Add Anyway" in the non-default-branch dialog.
    pub(crate) fn finish_add_project(
        &mut self,
        path: String,
        name: String,
        branch: String,
        leading_branch: String,
    ) -> Result<()> {
        let path_buf = PathBuf::from(&path);
        let display_name = if name.trim().is_empty() {
            path_buf
                .file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            name.trim().to_string()
        };
        let status_message = format!("Added project \"{display_name}\" to workspace");
        self.finish_add_project_with_status(path, name, branch, leading_branch, status_message)
    }

    pub(crate) fn finish_add_project_with_status(
        &mut self,
        path: String,
        name: String,
        branch: String,
        leading_branch: String,
        status_message: String,
    ) -> Result<()> {
        let path_buf = PathBuf::from(&path);
        let display_name = if name.trim().is_empty() {
            path_buf
                .file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            name.trim().to_string()
        };
        let project_id = Uuid::new_v4().to_string();
        let project = Project {
            id: project_id,
            name: display_name.clone(),
            path: path.clone(),
            explicit_default_provider: None,
            default_provider: self.engine.config.default_provider(),
            leading_branch: Some(leading_branch),
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
            current_branch: branch,
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: Some(chrono::Utc::now()),
        };
        logger::info(&format!("registered project {}", path_buf.display()));
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Add {
                project,
                status_message,
            }),
            // Add is inline (returns its final immediately); no handler-resolved op.
            status_op_id: None,
        })?;
        // The add is INLINE now: the reaction already carries the FINAL status
        // (the success info from the `Added` arm, or the rollback error). A
        // trailing `set_busy` here would run last and never resolve, leaving a
        // stuck spinner — so apply the reaction and stop.
        self.apply_reaction(reaction);
        Ok(())
    }

    /// `new-agent` / `n`: open the project chooser to pick which project the new
    /// agent belongs to. The flat agent list has no project header to select, so
    /// every project (agent-less included) is reachable only through the chooser.
    pub(crate) fn create_agent_for_selected_project(&mut self) -> Result<()> {
        self.open_project_chooser(ProjectChooserIntent::NewAgent)
    }

    /// Per-project body for `NewAgent`: kicks off branch inspection, which then
    /// opens the name prompt. Shared by the chooser and any direct selection
    /// path so the creation logic lives in exactly one place.
    pub(crate) fn begin_new_agent_for_project(&mut self, project: Project) -> Result<()> {
        // Close the chooser (or any prior prompt) before dispatching; the name
        // prompt is opened later by the branch-inspection completion handler.
        self.prompt = PromptState::None;
        if project.path_missing {
            return Ok(());
        }
        self.dispatch_create_agent_branch_inspection(project);
        Ok(())
    }

    /// Build one chooser row per project from the live engine state, counting the
    /// sessions that belong to each. Runtime-derived, display-only.
    pub(crate) fn build_project_chooser_entries(&self) -> Vec<ProjectChooserEntry> {
        self.engine
            .projects
            .iter()
            .map(|project| {
                let agent_count = self
                    .engine
                    .sessions
                    .iter()
                    .filter(|session| session.project_id() == Some(project.id.as_str()))
                    .count();
                ProjectChooserEntry {
                    id: project.id.clone(),
                    name: project.name.clone(),
                    path: project.path.clone(),
                    agent_count,
                    path_missing: project.path_missing,
                }
            })
            .collect()
    }

    /// Open the project chooser for the given intent. With zero projects there is
    /// nothing to pick, so this sets a helpful error instead of showing an empty
    /// modal. The gh availability gate for the PR intent is enforced by the
    /// caller (`open_new_agent_from_pr_prompt`) before we get here.
    pub(crate) fn open_project_chooser(&mut self, intent: ProjectChooserIntent) -> Result<()> {
        self.open_project_chooser_over(intent, None)
    }

    /// [`Self::open_project_chooser`] optionally narrowed to a set of project
    /// ids. `Some` is used when a pull-request reference matched SEVERAL
    /// projects: showing every project there would bury the two that are
    /// actually checkouts of that repository.
    pub(crate) fn open_project_chooser_over(
        &mut self,
        intent: ProjectChooserIntent,
        only: Option<&[String]>,
    ) -> Result<()> {
        let mut entries = self.build_project_chooser_entries();
        if let Some(only) = only {
            entries.retain(|entry| only.iter().any(|id| id == &entry.id));
        }
        if entries.is_empty() {
            self.set_error("No projects yet. Add one first.");
            return Ok(());
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::PickProject {
            intent,
            entries,
            list: SearchableList::new(),
        };
        Ok(())
    }

    /// Confirm the highlighted project in the chooser and dispatch by intent. An
    /// empty list is a no-op; a vanished project surfaces an error. `Manage`
    /// stores the pick as the project-action context and closes the modal.
    pub(crate) fn confirm_project_chooser_selection(&mut self) -> Result<()> {
        let (intent, project_id) = match &self.prompt {
            PromptState::PickProject {
                intent,
                entries,
                list,
            } => {
                // `list.selected` indexes the visible list; resolve to an entry.
                let visible = list.visible_indices(entries, pick_project_matches);
                match visible.get(list.selected).and_then(|i| entries.get(*i)) {
                    Some(entry) => (*intent, entry.id.clone()),
                    None => return Ok(()),
                }
            }
            _ => return Ok(()),
        };
        let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
        else {
            self.prompt = PromptState::None;
            self.set_error("That project is no longer available.");
            return Ok(());
        };
        match intent {
            ProjectChooserIntent::NewAgent => self.begin_new_agent_for_project(project),
            ProjectChooserIntent::FromPr => {
                // Whatever the user had typed before stepping out to choose a
                // project travels back into the field, so the round trip never
                // costs them their reference.
                let seed = self.pending_pr_reference.take().unwrap_or_default();
                self.begin_pr_agent_for_project_with(project, seed)
            }
            ProjectChooserIntent::FromPrReference => {
                // The reference is already typed, so this pick completes it
                // rather than reopening an empty field. If it somehow went
                // missing, fall back to the project-first field instead of
                // silently doing nothing.
                match self.pending_pr_reference.take() {
                    Some(raw_input) => self.dispatch_pull_request_lookup(project, raw_input),
                    None => self.begin_pr_agent_for_project(project),
                }
            }
            ProjectChooserIntent::FromWorktree => self.begin_worktree_agent_for_project(project),
            ProjectChooserIntent::ManageWorktrees => {
                self.begin_manage_worktrees_for_project(project)
            }
            ProjectChooserIntent::Manage => {
                self.project_chooser_context = Some(project.id.clone());
                self.prompt = PromptState::None;
                self.set_info(format!(
                    "Project \"{}\" is now the target for project actions.",
                    project.name
                ));
                Ok(())
            }
            ProjectChooserIntent::ProjectTerminal => {
                self.prompt = PromptState::None;
                self.show_project_terminal(&project)
            }
        }
    }

    pub(crate) fn continue_create_agent_after_branch_inspection(
        &mut self,
        mut project: Project,
        inspection: CreateAgentBranchInspection,
    ) -> Result<()> {
        project.current_branch = inspection.current_branch;
        project.leading_branch = Some(inspection.leading_branch);
        project.branch_status =
            if project.leading_branch.as_deref() == Some(&project.current_branch) {
                ProjectBranchStatus::Leading
            } else {
                ProjectBranchStatus::NotLeading
            };
        self.open_name_new_agent_prompt(CreateAgentRequest::NewProject {
            project,
            custom_name: None,
            use_existing_branch: false,
            pull_before_create: self
                .engine
                .config
                .defaults
                .pull_before_creating_agent_by_default,
            copy_uncommitted_changes: self
                .engine
                .config
                .defaults
                .copy_uncommitted_changes_by_default,
        })
    }

    /// `new-agent-from-worktree`: open the project chooser, then (per project)
    /// load that project's worktrees into `PickProjectWorktree`.
    pub(crate) fn create_agent_from_existing_worktree(&mut self) -> Result<()> {
        self.open_project_chooser(ProjectChooserIntent::FromWorktree)
    }

    /// Per-project body for `FromWorktree`: opens the worktree picker and spawns
    /// the worktrees loader. Shared by the chooser and any direct selection path.
    pub(crate) fn begin_worktree_agent_for_project(&mut self, project: Project) -> Result<()> {
        if project.path_missing {
            self.prompt = PromptState::None;
            self.set_warning(format!("Project path not found: {}", project.path));
            return Ok(());
        }

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::PickProjectWorktree(PickProjectWorktreePrompt {
            project: project.clone(),
            entries: Vec::new(),
            loading: true,
            selected: None,
            error: None,
        });
        // Declare the loading→final states together. The final is decided in the
        // completion handler (it depends on whether the picker is still open and
        // matching when the worktrees arrive, which the worker can't see), so use
        // a HandlerStatusOp with a 3-way outcome. The failure name matches the
        // handler's prompt name (same project, resolved here at dispatch).
        let project_name = project.name.clone();
        let op = dux_core::engine::status_op("Loading git worktrees for the selected project...")
            .resolve_in_handler(move |o: &WorktreesFinalOutcome| match o {
                WorktreesFinalOutcome::Loaded => dux_core::engine::Final::info(
                    "Choose an available worktree to launch a new agent.",
                ),
                WorktreesFinalOutcome::Failed(error) => dux_core::engine::Final::error(format!(
                    "Failed to load worktrees for project \"{project_name}\": {error}"
                )),
                WorktreesFinalOutcome::Dismissed => dux_core::engine::Final::clear(),
            });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_worktree_ops.insert(op_id.clone(), op);
        self.engine
            .spawn_project_worktrees_worker(project, Some(op_id));
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn fork_selected_session(&mut self) -> Result<()> {
        let Some(source_session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first to fork.");
            return Ok(());
        };
        // Fork is agent-scoped: the new worktree belongs to the SAME project as
        // the agent being forked. Derive it from the source session, not from
        // `selected_project()`, which can resolve to a `manage-projects` target
        // pointing at a different project.
        let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|p| Some(p.id.as_str()) == source_session.project_id())
            .cloned()
        else {
            self.set_error("Select an agent session first to fork.");
            return Ok(());
        };
        let source_label = self.session_label(&source_session);

        self.open_name_new_agent_prompt(CreateAgentRequest::ForkSession {
            project,
            source_session: Box::new(source_session),
            source_label,
            custom_name: None,
        })
    }

    /// `new-agent-from-pr`: fail fast if gh integration is unavailable, then open
    /// the reference field with NO project chosen and none asked for. dux works
    /// out which project the reference belongs to; the secondary action inside
    /// the modal is the way back to today's project-first flow.
    pub(crate) fn open_new_agent_from_pr_prompt(&mut self) -> Result<()> {
        if !self.github_pr_agent_command_available() {
            self.set_error(
                "GitHub PR agent creation requires GitHub integration and an authenticated gh CLI.",
            );
            return Ok(());
        }
        self.invalidate_pull_request_resolution();
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.pending_pr_reference = None;
        self.prompt = PromptState::PullRequestInput {
            project: None,
            input: TextInput::new(),
            focus: PullRequestInputFocus::Input,
        };
        self.set_info(
            "Paste a pull request link, or type owner/repo#123. dux finds the project it belongs to.",
        );
        Ok(())
    }

    /// Per-project body for `FromPr`: opens the PR-number/URL input for a
    /// project that has ALREADY been chosen, which is the project-first flow
    /// unchanged. Reached from the project chooser and from the secondary
    /// action inside the reference-first modal.
    pub(crate) fn begin_pr_agent_for_project(&mut self, project: Project) -> Result<()> {
        self.begin_pr_agent_for_project_with(project, String::new())
    }

    /// [`Self::begin_pr_agent_for_project`] seeding the field with text the user
    /// has already typed, so stepping out to the project picker and back never
    /// throws their reference away.
    pub(crate) fn begin_pr_agent_for_project_with(
        &mut self,
        project: Project,
        seed: String,
    ) -> Result<()> {
        if project.path_missing {
            self.prompt = PromptState::None;
            self.set_warning(format!(
                "Cannot create an agent from a PR: path not found for \"{}\"",
                project.name
            ));
            return Ok(());
        }
        // Retargeting the modal at a project supersedes any resolution still
        // out: its answer is about a question this screen is no longer asking.
        self.invalidate_pull_request_resolution();
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        let mut input = TextInput::new();
        if !seed.is_empty() {
            input.set_text(seed);
        }
        self.prompt = PromptState::PullRequestInput {
            project: Some(project),
            input,
            focus: PullRequestInputFocus::Input,
        };
        self.set_info("Paste a GitHub PR URL or enter a PR number for the chosen project.");
        Ok(())
    }

    /// Confirm on the reference field with NO project chosen: parse what was
    /// typed, then resolve it against every project's configured address.
    ///
    /// Parsing happens HERE, inline, because it is pure and instant and its
    /// refusals are the ones the user most needs immediately: a bare number
    /// names no repository at all, so it is refused with a pointer at the
    /// secondary action rather than sent off to a worker that could only come
    /// back empty-handed. Only a reference that really names a repository is
    /// worth a git call per project, and that goes on a worker.
    pub(crate) fn dispatch_pull_request_reference(&mut self, raw_input: String) -> Result<()> {
        let reference = match dux_core::pr_reference::parse_typed_reference(&raw_input) {
            Ok(reference) => reference,
            Err(message) => {
                self.set_error(message);
                return Ok(());
            }
        };
        if reference.owner_repo.is_none() {
            self.set_error(
                "A pull request number on its own does not say which repository it is in. \
                 Paste a link, type owner/repo#123, or choose an existing project first.",
            );
            return Ok(());
        }
        let policy = self.engine.github_host_policy();
        // The typed host is gated BEFORE any per-project git work, exactly as
        // the web gates it. Without this a reference on a host `gh` is not
        // signed in to matched nothing (every project is on some other host),
        // so the user was told no project in dux had that repository, sent to
        // the picker, made to choose one, and only then shown the real
        // authentication error. The first message dux shows should be the true
        // one.
        if let Some(host) = reference.host.as_deref()
            && !policy.allows(host)
        {
            self.set_error(format!(
                "dux cannot look up pull requests on {host}. Sign in to that host with \
                 `gh auth login --hostname {host}`, or paste a reference from a host you \
                 are already signed in to."
            ));
            return Ok(());
        }
        let Some(repository) = reference.repository_label() else {
            self.set_error("That reference does not name a repository.");
            return Ok(());
        };

        // A resubmit supersedes whatever was already out: the old reply must
        // not be allowed to act on this screen.
        self.invalidate_pull_request_resolution();
        self.prompt = PromptState::None;
        let op =
            dux_core::engine::status_op(format!("Looking for the project for {repository}..."))
                .resolve_in_handler(|o: &PrLookupFinalOutcome| match o {
                    PrLookupFinalOutcome::HandedOff | PrLookupFinalOutcome::Failed => {
                        dux_core::engine::Final::clear()
                    }
                });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_pr_lookup_ops.insert(op_id.clone(), op);
        // The op id IS the generation stamp. It is already unique per
        // operation and already rides through the worker and back, so there is
        // nothing to invent: a reply whose id is no longer the current one
        // belongs to a screen the user has left.
        self.pending_pr_reference_op = Some(op_id.clone());
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));

        let worker_tx = self.engine.worker_tx.clone();
        let projects = self.engine.projects.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let tx_panic = worker_tx.clone();
            let op_id_panic = op_id.clone();
            let repository_panic = repository.clone();
            let raw_panic = raw_input.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::pr_reference::run_reference_resolution_job(
                    reference,
                    raw_input,
                    projects,
                    policy,
                    worker_tx,
                    Some(op_id),
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!(
                    "pull-request-reference resolution worker panicked: {reason}"
                ));
                // A panic must still complete the event, or the busy strands
                // and the modal never comes back. It is reported as a FAILURE,
                // not as an empty match set: dux never found out whether any
                // project is a checkout of that repository, and saying it did
                // would be a lie the user cannot see through.
                let _ = tx_panic.send(WorkerEvent::PullRequestReferenceResolved {
                    raw_input: raw_panic,
                    repository: repository_panic,
                    result: Err(reason),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(())
    }

    /// Forget the resolution this screen was waiting for, so its reply (which
    /// may already be in flight and cannot be recalled) lands on nothing, and
    /// dismiss its busy rather than leaving a spinner over a screen that is no
    /// longer waiting for anything.
    ///
    /// Called on every close, retarget and resubmit. An abort mechanism would
    /// be a fine addition on top; it could never replace this, because a reply
    /// already on the channel still arrives.
    pub(crate) fn invalidate_pull_request_resolution(&mut self) {
        let Some(op_id) = self.pending_pr_reference_op.take() else {
            return;
        };
        if let Some(op) = self.pending_pr_lookup_ops.remove(&op_id) {
            self.apply_reaction(op.resolve(&PrLookupFinalOutcome::HandedOff).into_reaction());
        }
    }

    /// What the resolution worker's answer means on screen. Three shapes, and
    /// every one of them keeps the reference the user typed. A worker that fell
    /// over is a fourth, and it is reported as a failure rather than folded
    /// into "no project".
    pub(crate) fn apply_pull_request_reference_resolution(
        &mut self,
        raw_input: String,
        repository: String,
        result: Result<dux_core::pr_reference::ReferenceResolution, String>,
    ) -> Result<()> {
        let resolution = match result {
            Ok(resolution) => resolution,
            Err(reason) => {
                self.set_error(format!(
                    "dux could not work out which project {repository} is open in: {reason}. \
                     Try again, or choose an existing project."
                ));
                return Ok(());
            }
        };
        let matches = &resolution.matches;
        match matches.len() {
            1 => {
                let project = matches[0].clone();
                self.dispatch_pull_request_lookup(project, raw_input)
            }
            0 => {
                self.pending_pr_reference = Some(raw_input);
                // What dux may claim depends on whether it managed to look at
                // everything. With a project it could not inspect, "no project
                // is a checkout of this" is a certainty dux does not have, and
                // the one project that mattered may be exactly the unreadable
                // one. dux does not clone, and neither wording may imply it
                // might.
                match resolution.uninspected_summary() {
                    None => self.set_warning(format!(
                        "No project in dux is a checkout of {repository}. Choose a project that \
                         already has it, or add one from a directory on disk."
                    )),
                    Some(summary) => self.set_warning(format!(
                        "No project dux could check is a checkout of {repository}, and dux \
                         could not check every project ({summary}). Choose a project that \
                         already has it, or add one from a directory on disk."
                    )),
                }
                self.open_project_chooser_over(ProjectChooserIntent::FromPrReference, None)
            }
            _ => {
                let ids: Vec<String> = matches.iter().map(|p| p.id.clone()).collect();
                let count = ids.len();
                self.pending_pr_reference = Some(raw_input);
                self.set_info(format!(
                    "{count} projects are checkouts of {repository}. Choose which one this \
                     agent belongs in."
                ));
                self.open_project_chooser_over(ProjectChooserIntent::FromPrReference, Some(&ids))
            }
        }
    }

    pub(crate) fn dispatch_pull_request_lookup(
        &mut self,
        project: Project,
        raw_input: String,
    ) -> Result<()> {
        #[cfg(test)]
        self.dispatched_pr_lookups
            .push((project.id.clone(), raw_input.clone()));
        self.prompt = PromptState::None;
        // Mint a HandlerStatusOp keyed by an opaque id. Its busy shows now; both
        // terminal outcomes resolve to a CLEAR in `drain_events` when the
        // `PullRequestResolved` event returns carrying this id. The visible final
        // comes from elsewhere (the name prompt's `set_info` on success, the
        // engine's error `Status` on failure), so the op only DISMISSES its busy
        // — but keying it guarantees the spinner is replaced rather than stranding
        // to the busy timeout. The id rides through the lookup worker and back.
        let op = dux_core::engine::status_op(format!(
            "Resolving PR for project \"{}\"...",
            project.name
        ))
        .resolve_in_handler(|o: &PrLookupFinalOutcome| match o {
            PrLookupFinalOutcome::HandedOff | PrLookupFinalOutcome::Failed => {
                dux_core::engine::Final::clear()
            }
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_pr_lookup_ops.insert(op_id.clone(), op);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        let worker_tx = self.engine.worker_tx.clone();
        let policy = self.engine.github_host_policy();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // The TUI resolves the PR first and then prompts for a name, so it
            // carries no custom name through the lookup (the prompt seeds the
            // head branch as the default).
            //
            // `worker_tx` is moved into the job; `tx_panic` is kept outside
            // `catch_unwind` so it remains valid if the job panics.
            let tx_panic = worker_tx.clone();
            let op_id_panic = op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::gh::run_pull_request_lookup_job(
                    project,
                    raw_input,
                    None,
                    worker_tx,
                    Some(op_id),
                    policy,
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!("pull-request-lookup worker panicked: {reason}"));
                let _ = tx_panic.send(WorkerEvent::PullRequestResolved {
                    result: Err(format!("Worker panicked: {reason}")),
                    purpose: dux_core::worker::PrLookupPurpose::CreateAgent,
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(())
    }

    pub(crate) fn open_name_new_agent_prompt(&mut self, request: CreateAgentRequest) -> Result<()> {
        let initial_name = match &request {
            CreateAgentRequest::NewProject { custom_name, .. }
            | CreateAgentRequest::ForkSession { custom_name, .. }
            | CreateAgentRequest::ForkExternalWorktree { custom_name, .. } => custom_name.clone(),
            // A standalone create already has its title (resolved from the
            // folder), so the prompt opens pre-filled with it.
            CreateAgentRequest::Standalone { title, .. } => Some(title.clone()),
            CreateAgentRequest::PullRequest {
                custom_name,
                head_branch,
                ..
            } => custom_name.clone().or_else(|| Some(head_branch.clone())),
            CreateAgentRequest::ExistingManagedWorktree {
                custom_name,
                worktree_path,
                ..
            } => custom_name.clone().or_else(|| {
                worktree_path
                    .file_name()
                    .and_then(|part| part.to_str())
                    .map(str::to_string)
            }),
        };
        let randomize_name = initial_name.is_none()
            && self
                .engine
                .config
                .defaults
                .enable_randomized_pet_name_by_default;
        let mut input = TextInput::new().with_char_map(crate::git::agent_name_char_map);
        let mut randomized_name = None;
        if let Some(name) = initial_name {
            input.set_text(name);
        } else if randomize_name {
            let name = crate::git::docker_style_name();
            input.set_text(name.clone());
            randomized_name = Some(name);
        }

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::NameNewAgent {
            request,
            input,
            randomize_name,
            randomized_name,
            copy_changes: self
                .engine
                .config
                .defaults
                .copy_uncommitted_changes_by_default,
            focus: NameNewAgentFocus::Input,
        };
        Ok(())
    }

    pub(crate) fn open_name_new_agent_prompt_for_request(
        &mut self,
        request: CreateAgentRequest,
    ) -> Result<()> {
        self.open_name_new_agent_prompt(request)
    }

    /// Spawns a background worker that runs `git switch <target_branch>` in
    /// the source repo before registering the project. On success, the
    /// `WorkerEvent::NonDefaultBranchCheckoutCompleted` handler continues the
    /// selected action; on failure it surfaces the git error.
    ///
    /// `carried_op_id` lets the checkout-default-branch chain (worker 1) keep ONE
    /// `pending_checkout_inspect_ops` op spanning the inspect→switch sequence: when
    /// `Some`, the op already lives in the map and its busy text was already
    /// re-emitted as a `progress` by the chain handler, so this only forwards the
    /// id into worker 2. When `None` (the standalone add-project / checkout-default
    /// entry points), this mints a fresh op, shows its keyed busy, and stashes it.
    pub(crate) fn dispatch_non_default_branch_checkout(
        &mut self,
        action: NonDefaultBranchAction,
        target_branch: String,
        reason: String,
        carried_op_id: Option<String>,
    ) {
        let path = action.repo_path().to_string();
        let status_op_id = match carried_op_id {
            Some(id) => id,
            None => {
                // The keyed busy is dismissed by the op's `Final::Clear` when the
                // worker reports back; the visible final (the engine's unkeyed
                // success/error `Status`, or the TUI's add-project view handler)
                // is authored elsewhere, byte-for-byte unchanged.
                let op = dux_core::engine::status_op(format!(
                    "Checking out \"{target_branch}\" in {path} {reason}..."
                ))
                .resolve_in_handler(|o: &TuiCheckoutInspectOutcome| match o {
                    TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
                });
                let pending = op.pending_status();
                let id = op.id().to_string();
                self.pending_checkout_inspect_ops.insert(id.clone(), op);
                self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
                id
            }
        };
        let worker_tx = self.engine.worker_tx.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            // Pre-clone the values needed for the panic-path event before
            // they are moved into the job closure.
            let tx_panic = worker_tx.clone();
            let action_panic = action.clone();
            let branch_panic = target_branch.clone();
            let op_id_panic = status_op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::project_browser::run_add_project_checkout_job(
                    action,
                    target_branch,
                    worker_tx,
                    Some(status_op_id),
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!(
                    "non-default-branch-checkout worker panicked: {reason}"
                ));
                let _ = tx_panic.send(WorkerEvent::NonDefaultBranchCheckoutCompleted {
                    action: action_panic,
                    target_branch: branch_panic,
                    result: Err(format!("Worker panicked: {reason}")),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
    }

    /// Dispatch the "create an empty initial commit, then add the project"
    /// flow to a background worker (the commit can hit slow filesystem/lock
    /// work, so it must stay off the UI thread). Serializes per repo path via
    /// `InFlightKey::InitialCommit` so a repeat confirm can't double-commit.
    /// Worker completion posts `InitialCommitCreated`, whose
    /// `AddProjectAfterInitialCommit` reaction registers the project.
    pub(crate) fn dispatch_create_initial_commit(&mut self, path: String, name: String) {
        // Real branch the commit will land on. Propagate a git error rather than
        // silently defaulting the branch (which would mis-tag the project and
        // break the next agent creation). We register on this branch directly and
        // DELIBERATELY skip the non-default-branch heuristic warning (`git init -b
        // trunk` etc.): for a repo the user just created, "this doesn't look like
        // main" is noise, not a helpful heads-up (contrast adding a pre-existing
        // repo, where the warning earns its keep).
        let branch = match git::current_branch_opt(Path::new(&path)) {
            Ok(b) => b.unwrap_or_default(),
            Err(e) => {
                self.set_error(format!(
                    "Couldn't read the current branch of \"{path}\": {e:#}"
                ));
                return;
            }
        };
        let leading_branch = leading_branch_for_project(
            Path::new(&path),
            (!branch.is_empty()).then_some(branch.as_str()),
        );
        // Fail-closed commit state, mirroring the web handler: only a confirmed
        // unborn repo goes through the bootstrap worker. If a commit raced in
        // since the dialog opened, register it directly; if git can't say, stop.
        match git::repo_commit_state(Path::new(&path)) {
            git::CommitState::Unborn => {}
            git::CommitState::Born => {
                if let Err(e) = self.finish_add_project(path, name, branch, leading_branch) {
                    self.set_error(format!("{e:#}"));
                }
                return;
            }
            git::CommitState::Indeterminate => {
                self.set_error(format!(
                    "Couldn't determine the commit state of \"{path}\"; not creating an initial commit. Check the repository and retry."
                ));
                return;
            }
        }
        if !self
            .engine
            .mark_in_flight(dux_core::engine::InFlightKey::InitialCommit(path.clone()))
        {
            self.set_warning(format!(
                "An initial commit is already being created for \"{path}\". Please wait for it to finish."
            ));
            return;
        }
        let add = dux_core::worker::InitialCommitAdd {
            path: path.clone(),
            name,
            branch,
            leading_branch,
            initialized_repo: false,
            seeded_gitignore: false,
            seed_warning: None,
        };
        // Keyed busy dismissed by the op's `Final::Clear` when the worker reports
        // back (see `drain_events`); the visible final is the add-project view
        // handler's success message or the engine's error `Status`.
        let op = dux_core::engine::status_op(format!(
            "Creating an initial commit in {path} before adding the project..."
        ))
        .resolve_in_handler(|o: &TuiCheckoutInspectOutcome| match o {
            TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
        });
        let pending = op.pending_status();
        let status_op_id = op.id().to_string();
        self.pending_checkout_inspect_ops
            .insert(status_op_id.clone(), op);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        let worker_tx = self.engine.worker_tx.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let tx_panic = worker_tx.clone();
            let add_panic = add.clone();
            let op_id_panic = status_op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::project_browser::run_create_initial_commit_job(
                    add,
                    worker_tx,
                    Some(status_op_id),
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!("initial-commit worker panicked: {reason}"));
                let _ = tx_panic.send(WorkerEvent::InitialCommitCreated {
                    add: add_panic,
                    result: Err(format!("Worker panicked: {reason}")),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
    }

    /// Dispatch the adopt-a-folder flow (git init, seed a starter .gitignore,
    /// create the initial commit, then add the project) to a background
    /// worker, mirroring `dispatch_create_initial_commit`. Shares
    /// `InFlightKey::InitialCommit` so init-and-commit and commit-only on the
    /// same path are mutually exclusive. Worker completion posts
    /// `InitialCommitCreated`, whose `AddProjectAfterInitialCommit` reaction
    /// registers the project.
    pub(crate) fn dispatch_init_repo(&mut self, path: String, name: String) {
        if !self
            .engine
            .mark_in_flight(dux_core::engine::InFlightKey::InitialCommit(path.clone()))
        {
            self.set_warning(format!(
                "A repository is already being initialized in \"{path}\". Please wait for it to finish."
            ));
            return;
        }
        let add = dux_core::worker::InitialCommitAdd {
            path: path.clone(),
            name,
            // The worker resolves the real branch after the commit lands;
            // these placeholders are rewritten by `init_repo_and_commit`.
            branch: String::new(),
            leading_branch: String::new(),
            initialized_repo: false,
            seeded_gitignore: false,
            seed_warning: None,
        };
        // Keyed busy dismissed by the op's `Final::Clear` when the worker
        // reports back (see `drain_events`); the visible final is the
        // add-project view handler's success message or the engine's error.
        let op = dux_core::engine::status_op(format!(
            "Initializing a git repository in {path} before adding the project..."
        ))
        .resolve_in_handler(|o: &TuiCheckoutInspectOutcome| match o {
            TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
        });
        let pending = op.pending_status();
        let status_op_id = op.id().to_string();
        self.pending_checkout_inspect_ops
            .insert(status_op_id.clone(), op);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        let worker_tx = self.engine.worker_tx.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let tx_panic = worker_tx.clone();
            let add_panic = add.clone();
            let op_id_panic = status_op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::project_browser::run_init_repo_job(add, worker_tx, Some(status_op_id));
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!(
                    "repository-initialization worker panicked: {reason}"
                ));
                let _ = tx_panic.send(WorkerEvent::InitialCommitCreated {
                    add: add_panic,
                    result: Err(format!("Worker panicked: {reason}")),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
    }

    pub(crate) fn dispatch_create_agent_branch_inspection(&mut self, project: Project) {
        // The keyed busy is dismissed by the op's `Final::Clear` when
        // `CreateAgentBranchInspected` returns carrying this id; the visible final
        // is authored elsewhere (the `ContinueCreateAgentAfterInspection` view
        // handler's `set_info` on success, the engine's error `Status` on failure),
        // byte-for-byte unchanged.
        let op = dux_core::engine::status_op(format!(
            "Checking the current branch for project \"{}\" before creating an agent...",
            project.name
        ))
        .resolve_in_handler(|o: &TuiCheckoutInspectOutcome| match o {
            TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
        });
        let pending = op.pending_status();
        let status_op_id = op.id().to_string();
        self.pending_checkout_inspect_ops
            .insert(status_op_id.clone(), op);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        let worker_tx = self.engine.worker_tx.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let tx_panic = worker_tx.clone();
            let project_panic = project.clone();
            let op_id_panic = status_op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                super::workers::run_create_agent_branch_inspection_job(
                    project,
                    worker_tx,
                    Some(status_op_id),
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!(
                    "create-agent-branch-inspection worker panicked for project \"{}\": {reason}",
                    project_panic.name
                ));
                let _ = tx_panic.send(WorkerEvent::CreateAgentBranchInspected {
                    project: project_panic,
                    result: Err(format!("Worker panicked: {reason}")),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
    }

    pub(crate) fn checkout_selected_project_default_branch(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        if project.path_missing {
            self.set_warning(format!(
                "Cannot check out default branch: path not found for \"{}\"",
                project.name
            ));
            return Ok(());
        }

        // ONE op spans the whole chain. Worker 1's short-circuit terminals
        // (already-leading / heuristic / inspect-failed) resolve it to a clear in
        // `drain_events` (the engine's unkeyed `Status` carries the visible
        // message); the Known case forwards this id into worker 2 and re-emits the
        // busy text via `progress`, so the spinner is continuous with changing text
        // until worker 2's `NonDefaultBranchCheckoutCompleted` clears it.
        let op = dux_core::engine::status_op(format!(
            "Checking the default branch for project \"{}\"...",
            project.name
        ))
        .resolve_in_handler(|o: &TuiCheckoutInspectOutcome| match o {
            TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
        });
        let pending = op.pending_status();
        let status_op_id = op.id().to_string();
        self.pending_checkout_inspect_ops
            .insert(status_op_id.clone(), op);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        let worker_tx = self.engine.worker_tx.clone();
        thread::spawn(move || {
            use std::panic::AssertUnwindSafe;
            let tx_panic = worker_tx.clone();
            let project_panic = project.clone();
            let op_id_panic = status_op_id.clone();
            if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| {
                dux_core::project_browser::run_checkout_project_default_branch_inspection_job(
                    project,
                    worker_tx,
                    Some(status_op_id),
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!(
                    "checkout-default-branch-inspection worker panicked for project \"{}\": \
                     {reason}",
                    project_panic.name
                ));
                let _ = tx_panic.send(WorkerEvent::CheckoutProjectDefaultBranchInspected {
                    project: project_panic,
                    result: Err(format!("Worker panicked: {reason}")),
                    status_op_id: Some(op_id_panic),
                });
            }
        });
        Ok(())
    }

    pub(crate) fn dispatch_create_agent_request(
        &mut self,
        request: CreateAgentRequest,
        busy_message: String,
    ) -> Result<()> {
        let term_size = crossterm::terminal::size().unwrap_or((80, 24));
        let reaction = self.engine.apply(Command::DispatchCreateAgentRequest {
            request: Box::new(request),
            busy_message,
            term_size,
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(crate) fn pty_size_for_launch(&self) -> (u16, u16) {
        if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        }
    }

    /// Create a fresh extra tab for `session_id` running `provider`, focus it,
    /// and report the outcome via the status line. Shared by the
    /// new-agent-tab picker's single-provider skip and its Apply branch, so
    /// the status copy and focus behavior never drift between the entry
    /// points. Both new-tab entry points (the `new-agent-tab` palette command
    /// and the `Action::NewTab` key) route through
    /// `open_new_tab_provider_prompt` first, which calls this. The TUI strip
    /// deliberately draws no `+` button (see `render.rs`), so there is no
    /// third entry point.
    fn spawn_tab_with_provider(&mut self, session_id: &str, provider: ProviderKind) {
        let pty_size = self.pty_size_for_launch();
        match self.engine.create_tab(session_id, provider, pty_size) {
            Ok(tab_id) => {
                self.set_focused_tab(session_id, &tab_id);
                self.rebuild_left_items();
                self.set_info(
                    "Added a tab. It starts fresh — a new tab does not resume a prior conversation."
                        .to_string(),
                );
            }
            Err(e) => self.set_error(format!("Could not add tab: {e}")),
        }
    }

    /// Open the new-agent-tab provider picker (reuses `ChangeAgentProviderPrompt`
    /// in `NewTab` mode). Refuses at the per-agent tab cap with a keyed error
    /// status instead of opening the modal (`create_tab` also enforces the cap
    /// as a backstop). When exactly one provider is configured, skips the
    /// modal entirely and creates the tab directly with it: a one-option radio
    /// list is pure friction. That early branch is kept separate and clearly
    /// commented so it is trivial to flip if that decision changes.
    pub(crate) fn open_new_tab_provider_prompt(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        if self.engine.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }

        let max_per_agent = i64::from(self.engine.agent_tabs_max());
        let current_tabs = self.engine.session_store.count_agent_tabs(&session.id)?;
        if current_tabs + 1 >= max_per_agent {
            self.set_error(format!(
                "This agent already has the maximum of {max_per_agent} tabs. Close a tab before adding another."
            ));
            return Ok(());
        }

        let options = self.change_agent_provider_options(&session);

        // Single-provider skip (judgment call, documented in the doc comment
        // above): with exactly one configured provider a radio list is pure
        // friction, so create the tab directly with it.
        if options.len() == 1 {
            let provider = options[0].provider.clone();
            self.spawn_tab_with_provider(&session.id, provider);
            return Ok(());
        }

        // The single-source new-tab default provider (owning project else global
        // config default), shared with the web via `default_provider_for_new_tab`.
        let default_provider = self
            .engine
            .default_provider_for_new_tab(session.project_id());
        let selected = options
            .iter()
            .position(|option| option.provider == default_provider)
            .unwrap_or(0);

        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
            session_id: session.id.clone(),
            tab_id: session.id.clone(),
            session_label: self.session_label(&session),
            worktree_path: session.directory().to_string(),
            options,
            selected,
            mode: ChangeAgentProviderMode::NewTab,
        });
        self.set_info(
            "Choose a provider for the new tab. It starts fresh; a new tab does not resume a prior conversation.",
        );
        Ok(())
    }

    /// Launch a dormant focused tab. Used by the Enter/activate path when the
    /// focused tab has no live process (e.g. after a restart). Resume is decided
    /// per-provider: reopening resumes that provider's conversation when it is the
    /// sole live tab of that provider (see `tab_resume_decision`); otherwise fresh.
    pub(crate) fn launch_focused_support_tab(
        &mut self,
        _session_id: &str,
        tab_id: &str,
        seek_fullscreen: bool,
    ) -> Result<()> {
        // Resolution, per-provider resume decision, message wording, and the
        // request build are the single-source `Engine::dormant_tab_launch_request`
        // (shared with the web `launch_agent`) so the two surfaces cannot drift.
        // `None` (unknown tab / gone session) is a silent no-op, matching the
        // previous early return.
        let pty_size = self.pty_size_for_launch();
        if let Some(mut request) = self.engine.dormant_tab_launch_request(tab_id, pty_size) {
            request.wants_fullscreen = self.launch_seeks_fullscreen(seek_fullscreen);
            self.dispatch_agent_launch(request);
        }
        Ok(())
    }

    /// Whether a launch dispatched right now should land fullscreen on
    /// completion (decision 10). `seek_fullscreen` is the caller's explicit
    /// intent (the fullscreen toggle on a dormant tab); on top of that, any
    /// launch initiated while the fullscreen relaunch screen is up (e.g.
    /// ReconnectAgent pressed on the dormant fullscreen surface) keeps the
    /// user fullscreen rather than yanking them down to the 3-pane layout.
    pub(crate) fn launch_seeks_fullscreen(&self, seek_fullscreen: bool) -> bool {
        seek_fullscreen || !matches!(self.fullscreen_overlay, FullscreenOverlay::None)
    }

    /// Close-tab entry point. Opens the confirmation dialog for the focused
    /// tab: closing the session-slot tab detaches the agent (non-destructive); closing a
    /// extra tab ends that session for good (destructive).
    pub(crate) fn close_focused_tab_prompt(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let session_id = session.id.clone();
        let tab_id = self.focused_tab_id(&session_id);
        let is_main = tab_id == session_id;
        let provider = if is_main {
            session.provider.as_str().to_string()
        } else {
            self.engine
                .agent_tabs
                .get(&tab_id)
                .map(|t| t.provider.as_str().to_string())
                .unwrap_or_else(|| session.provider.as_str().to_string())
        };
        let provider_label = Self::title_case_word(&provider);
        self.prompt = PromptState::ConfirmCloseTab {
            session_id,
            tab_id,
            provider_label,
            is_main,
            focus: ConfirmFocus::Cancel,
        };
    }

    pub(crate) fn agent_launch_request(
        &self,
        session: AgentSession,
        resume: bool,
        kind: AgentLaunchKind,
    ) -> AgentLaunchRequest {
        self.engine
            .build_agent_launch_request(session, resume, self.pty_size_for_launch(), kind)
    }

    /// Build the keyed status op for a reconnect / fresh-restart launch. The
    /// resolver reads the terminal message straight off the launch reaction's
    /// [`dux_core::engine::LaunchOutcome`] (the engine computes the success line; the failure
    /// arms carry branch + message), so it captures no dispatch-time state and
    /// reproduces the TUI's exact wording for every outcome.
    pub(super) fn build_reconnect_status_op(
        &self,
        busy_message: String,
    ) -> dux_core::engine::HandlerStatusOp<dux_core::engine::LaunchOutcome> {
        dux_core::engine::status_op(busy_message).resolve_in_handler(
            |o: &dux_core::engine::LaunchOutcome| dux_core::engine::launch_outcome_final(o),
        )
    }

    pub(crate) fn dispatch_agent_launch(&mut self, request: AgentLaunchRequest) -> bool {
        let reaction = match self.engine.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        }) {
            Ok(r) => r,
            Err(e) => {
                self.set_error(format!("{e:#}"));
                return false;
            }
        };
        let launched = matches!(
            &reaction,
            EventReaction::DispatchAgentLaunchView(view) if view.launched
        );
        self.apply_reaction(reaction);
        launched
    }

    pub(crate) fn show_agent_surface(&mut self) {
        self.focus = FocusPane::Center;
        self.center_mode = CenterMode::Agent;
        self.session_surface = SessionSurface::Agent;
        self.fullscreen_overlay = FullscreenOverlay::None;
    }

    /// Landing for a COMPLETED agent launch (decision 10): fullscreen only
    /// when the launch was fullscreen-seeking (the request's
    /// `wants_fullscreen` bit, stamped at dispatch); every other launch lands
    /// focused-but-minimized so the center pane is immediately typeable.
    /// Callers run `show_agent_surface` first, which already put focus on the
    /// Center agent surface with no overlay.
    pub(crate) fn land_completed_launch(&mut self, wants_fullscreen: bool) {
        if wants_fullscreen {
            self.input_target = InputTarget::Agent;
            self.fullscreen_overlay = FullscreenOverlay::Agent;
        } else {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
        }
    }

    /// Extend an engine-composed launch-completion message with the TUI's
    /// landing note. The engine's message is shared with the web (which has no
    /// modes and no keybindings), so the note about where the launch landed
    /// and how to go fullscreen is appended TUI-side, with the key resolved
    /// through the bindings. A fullscreen-seeking launch gets the opposite
    /// note: it landed fullscreen, and the same toggle is the way back.
    pub(crate) fn launch_completion_message(
        &self,
        engine_message: String,
        wants_fullscreen: bool,
    ) -> String {
        let key = self.bindings.label_for(Action::ToggleFullscreen);
        if wants_fullscreen {
            format!("{engine_message} The pane is fullscreen; press {key} to minimize.")
        } else {
            format!(
                "{engine_message} The pane is focused, so you can type to the agent right away; press {key} for fullscreen."
            )
        }
    }

    pub(crate) fn show_companion_terminal_surface(&mut self) {
        self.session_surface = SessionSurface::Terminal;
        self.fullscreen_overlay = FullscreenOverlay::Terminal;
    }

    /// Always spawns a new companion terminal for the selected session.
    pub(crate) fn show_companion_terminal(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };

        // Route through the shared core creator so the id mint, the "Terminal N"
        // identity label, and the monotonic `sort_order` stamp are single-sourced
        // with the web. The TUI used to hand-insert here with `sort_order: 1`,
        // which made the default drag order nondeterministic (HashMap iteration
        // order) and never assigned the identity label.
        let (rows, cols) = self.pty_size_for_launch();
        let terminal_id = match self
            .engine
            .create_companion_terminal(&session.id, rows, cols)
        {
            Ok((id, _label)) => id,
            Err(e) => {
                self.set_error(format!("Could not launch terminal: {e:#}"));
                return Ok(());
            }
        };
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched terminal for agent \"{}\".",
            session.display_label()
        ));
        Ok(())
    }

    /// Always spawns a new project terminal at the given project's repo root.
    /// A project terminal is a plain shell with no agent attached; it does NOT
    /// run the project's `startup_command`.
    pub(crate) fn show_project_terminal(&mut self, project: &Project) -> Result<()> {
        if project.path_missing {
            self.set_warning(format!(
                "Cannot open a project terminal: path not found for \"{}\".",
                project.name
            ));
            return Ok(());
        }
        // Shared core creator (see `show_companion_terminal`): single-sources the
        // id, the "Terminal N" label, and the deterministic `sort_order`.
        let (rows, cols) = self.pty_size_for_launch();
        let terminal_id = match self.engine.create_project_terminal(&project.id, rows, cols) {
            Ok((id, _label)) => id,
            Err(e) => {
                self.set_error(format!("Could not launch project terminal: {e:#}"));
                return Ok(());
            }
        };
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        // A project terminal keeps its project above the "no agents" separator,
        // so the sidebar grouping may have changed.
        self.rebuild_left_items();
        self.set_info(format!(
            "Launched project terminal at the repo root of \"{}\".",
            project.name
        ));
        Ok(())
    }

    /// Palette command (`new-standalone-terminal`): always spawns a new
    /// standalone terminal in the user's home directory.
    ///
    /// A standalone terminal belongs to nothing, so unlike the other two this
    /// needs nothing selected and nothing to exist: no agent, no project, not
    /// even a project on disk. It also runs no `startup_command`, for the same
    /// reason a project terminal does not.
    pub(crate) fn show_standalone_terminal(&mut self) -> Result<()> {
        // Shared core creator (see `show_companion_terminal`): single-sources the
        // id, the "Terminal N" label, and the deterministic `sort_order`.
        let (rows, cols) = self.pty_size_for_launch();
        let terminal_id = match self.engine.create_standalone_terminal(rows, cols) {
            Ok((id, _label)) => id,
            Err(e) => {
                self.set_error(format!("Could not launch standalone terminal: {e:#}"));
                return Ok(());
            }
        };
        let where_it_is =
            dux_core::home_path::shorten_home(&dux_core::home_path::standalone_terminal_dir());
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        // The lifetime, stated truthfully: dux's own shutdown closes every
        // terminal, so "nothing closes it but you" was an overstatement. What is
        // actually special about this kind is that no OTHER event does: removing
        // a project or deleting an agent closes their terminals and leaves this
        // one alone.
        self.set_info(format!(
            "Launched a standalone terminal in {where_it_is}. It belongs to no project and no agent, so it keeps running until it exits, you close it, or dux shuts down."
        ));
        Ok(())
    }

    /// Opens the first existing companion terminal for the SELECTED AGENT, or
    /// spawns a new one if none exists. Agent-scoped only: project terminals are
    /// reached through the explicit `new-terminal-for-project` command (via the
    /// project chooser), never by guessing from what is selected.
    pub(crate) fn show_or_open_first_terminal(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_warning(
                "Select an agent first, or use new-terminal-for-project for a project terminal.",
            );
            return Ok(());
        };
        let owner = TerminalOwner::Session(session.id.clone());

        let first = self
            .engine
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.owner == owner)
            .min_by_key(|(id, _)| {
                id.strip_prefix("term-")
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(u64::MAX)
            })
            .map(|(id, t)| (id.clone(), t.label.clone()));

        if let Some((terminal_id, label)) = first {
            self.active_terminal_id = Some(terminal_id);
            self.terminal_return_to_list = false;
            self.show_companion_terminal_surface();
            self.input_target = InputTarget::Terminal;
            self.set_info(format!("Opened terminal \"{label}\"."));
            return Ok(());
        }
        self.show_companion_terminal()
    }

    /// Spawns a new companion terminal for the owner (agent session or
    /// project) of the currently selected terminal in the terminals list.
    pub(crate) fn spawn_terminal_for_selected_terminal(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(_, terminal)) = items.get(self.selected_terminal_index) else {
            self.set_warning("No terminal selected.");
            return Ok(());
        };
        let owner = terminal.owner.clone();
        drop(items);

        let session_id = match owner {
            TerminalOwner::Session(session_id) => session_id,
            TerminalOwner::Project(project_id) => {
                // A project terminal's sibling is another project terminal.
                let Some(project) = self
                    .engine
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .cloned()
                else {
                    self.set_warning("The parent project no longer exists.");
                    return Ok(());
                };
                return self.show_project_terminal(&project);
            }
            // A standalone terminal's sibling is another standalone terminal.
            // There is no owner to resolve first, and none to have gone missing.
            TerminalOwner::Standalone => return self.show_standalone_terminal(),
        };

        let Some(session) = self
            .engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
        else {
            self.set_warning("The parent agent session no longer exists.");
            return Ok(());
        };

        // Shared core creator (see `show_companion_terminal`).
        let (rows, cols) = self.pty_size_for_launch();
        let terminal_id = match self
            .engine
            .create_companion_terminal(&session.id, rows, cols)
        {
            Ok((id, _label)) => id,
            Err(e) => {
                self.set_error(format!("Could not launch terminal: {e:#}"));
                return Ok(());
            }
        };
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched new terminal for agent \"{}\".",
            session.display_label()
        ));
        Ok(())
    }

    /// Palette command (`new-terminal-for-agent`): spawns a new companion
    /// terminal for the SELECTED AGENT only. Project terminals have their own
    /// explicit command (`new-terminal-for-project`) that routes through the
    /// project chooser, so this no longer guesses at a project when no agent is
    /// selected (which made project terminals unreachable with an agent
    /// selected). Uses a yellow warning when no agent is selected.
    pub(crate) fn new_companion_terminal(&mut self) -> Result<()> {
        if self.selected_session().is_some() {
            return self.show_companion_terminal();
        }
        self.set_warning(
            "Select an agent first, or use new-terminal-for-project for a project terminal.",
        );
        Ok(())
    }

    /// Opens the terminal overlay for the terminal selected in the terminals list.
    pub(crate) fn open_terminal_from_terminal_list(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(terminal_id, terminal)) = items.get(self.selected_terminal_index) else {
            return Ok(());
        };
        let terminal_id = terminal_id.clone();
        let owner = terminal.owner.clone();
        let label = terminal.label.clone();
        drop(items);

        // Select this terminal's owner (session or project) in the left pane.
        let pos = match &owner {
            TerminalOwner::Session(session_id) => self.left_items().iter().position(
                |item| matches!(item, LeftItem::Session(idx) if self.engine.sessions.get(*idx).map(|s| s.id.as_str()) == Some(session_id.as_str())),
            ),
            // The flat agent list has no project rows, so a project terminal has no
            // left-pane row to move the cursor onto. A standalone terminal has no
            // owner at all, so it has none either.
            TerminalOwner::Project(_) | TerminalOwner::Standalone => None,
        };
        if let Some(pos) = pos {
            self.selected_left = pos;
        }
        self.reload_changed_files();

        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = false;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!("Opened terminal \"{label}\"."));
        Ok(())
    }

    pub(crate) fn refresh_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        if project.path_missing {
            self.set_warning(format!(
                "Cannot refresh: path not found for \"{}\"",
                project.name
            ));
            return Ok(());
        }
        logger::info(&format!("refreshing project {}", project.path));
        let reaction = self.engine.apply(Command::Pull {
            repo_path: PathBuf::from(&project.path),
            target: PullTarget::Project {
                project_id: project.id,
                project_name: project.name.clone(),
                leading_branch: project.leading_branch.clone(),
            },
            busy_message: format!("Refreshing project \"{}\" from remote\u{2026}", project.name),
            already_running_message: format!(
                "Project refresh already in progress for \"{}\". Wait for the current pull to finish.",
                project.name,
            ),
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(crate) fn confirm_delete_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let target = match &session.workspace {
            dux_core::model::AgentWorkspace::Managed(managed) => {
                let worktree_shared = self
                    .engine
                    .sessions
                    .iter()
                    .any(|s| s.id != session.id && s.directory() == session.directory());
                crate::app::DeleteAgentTarget::Managed {
                    branch_name: managed.branch_name.clone(),
                    initial_branch: managed.initial_branch.clone(),
                    branch_provenance: managed.branch_provenance,
                    worktree_shared,
                }
            }
            dux_core::model::AgentWorkspace::Folder(folder) => {
                crate::app::DeleteAgentTarget::Folder {
                    folder_label: dux_core::home_path::shorten_home(std::path::Path::new(
                        &folder.folder_path,
                    )),
                }
            }
        };
        self.prompt = PromptState::ConfirmDeleteAgent {
            session_id: session.id.clone(),
            agent_label: session.display_label(),
            target,
            focus: DeleteAgentFocus::Cancel, // Cancel is the safe default
            delete_worktree: false,          // Opt-in destructive action
        };
        Ok(())
    }

    /// Delete the agent session identified by `session_id`, blocking the
    /// calling thread for any git work. Project deletion now cascades through
    /// the core `Command::DeleteProject`, so this thin wrapper survives only as
    /// a synchronous test entry point for the `Command::DoDeleteSession`
    /// behavior; production single-agent deletes go through
    /// [`begin_delete_session`] so git work runs off the UI thread.
    ///
    /// When `delete_worktree` is true AND no other sessions share the worktree,
    /// the git worktree and branch are removed first. If the git removal fails,
    /// the session record is preserved so the caller can retry without losing
    /// the agent. When `delete_worktree` is false, the worktree and branch
    /// are always preserved.
    #[cfg(test)]
    pub(crate) fn do_delete_session(
        &mut self,
        session_id: &str,
        delete_worktree: bool,
    ) -> Result<()> {
        let reaction = self.engine.apply(Command::DoDeleteSession {
            session_id: session_id.to_string(),
            delete_worktree,
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    /// Kick off deletion of `session_id` from the user-facing modal.
    ///
    /// When the git worktree needs to be removed, the `git worktree remove`
    /// call is dispatched to a background thread and the session record is
    /// left in place until the worker reports success via
    /// [`WorkerEvent::WorktreeRemoveCompleted`]. This keeps the UI responsive
    /// even when git stalls (slow disk, held lock, large worktree). When no
    /// git work is required the session is cleaned up synchronously — that
    /// path only touches in-memory state and SQLite, which is effectively
    /// instantaneous.
    /// Build the keyed status op for an async worktree deletion. The resolver
    /// captures the dispatch-time session facts (provider / project name / branch
    /// name / display name) — the session is still present at dispatch because
    /// cleanup is deferred until git succeeds — and reproduces the TUI's exact
    /// wording for every terminal [`TuiDeleteOutcome`].
    pub(super) fn build_delete_status_op(
        &self,
        session_id: &str,
        busy_message: String,
    ) -> dux_core::engine::HandlerStatusOp<TuiDeleteOutcome> {
        let (provider, branch_name, initial_branch, name, project_name) = self
            .engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| {
                let provider = s.provider.as_str().to_string();
                // A standalone agent has no branch, and none of the
                // branch-naming arms below can be reached for one: its delete
                // resolves to the folder outcome, whose copy names the folder
                // instead. Empty strings here are therefore unreachable
                // placeholders, not values any sentence renders.
                let branch_name = s.branch_name().unwrap_or_default().to_string();
                // Captured here, with the session still present, because the
                // removal's report can name a SECOND branch (the one the agent
                // was born on) and the session is gone by the time it lands.
                let initial_branch = s.initial_branch().unwrap_or_default().to_string();
                let name = s.display_label();
                let project_name = s
                    .project_id()
                    .and_then(|project_id| self.engine.projects.iter().find(|p| p.id == project_id))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (provider, branch_name, initial_branch, name, project_name)
            })
            .unwrap_or_else(|| {
                (
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    "<unknown>".to_string(),
                )
            });
        dux_core::engine::status_op(busy_message).resolve_in_handler(
            move |o: &TuiDeleteOutcome| match o {
                // The keep path: nothing was deleted, so the line names the
                // branches that stayed and why, plus the manual way out (the
                // worktree is gone, so no dux surface can reach them now).
                TuiDeleteOutcome::SucceededPresent {
                    branches: dux_core::engine::RemovedBranches::Kept(provenance),
                } => dux_core::engine::Final::info(format!(
                    "Deleted {provider} agent \"{branch_name}\" and removed its worktree. {}",
                    provenance.kept_branches_note(&branch_name, &initial_branch)
                )),
                TuiDeleteOutcome::SucceededPresent {
                    branches: dux_core::engine::RemovedBranches::Deleted(branches),
                } => {
                    let base = match &branches.branch {
                        dux_core::git::BranchDeletion::Deleted => format!(
                            "Deleted {provider} agent from project \"{project_name}\" with branch \"{branch_name}\"."
                        ),
                        dux_core::git::BranchDeletion::AlreadyGone => format!(
                            "Deleted agent (branch \"{branch_name}\" was already removed)."
                        ),
                        // git refused, so the branch is STILL THERE: say so,
                        // give git's reason, and name the way out.
                        dux_core::git::BranchDeletion::Refused { reason } => format!(
                            "Deleted agent, but its branch \"{branch_name}\" is still there. {}",
                            dux_core::git::branch_refusal_note(&branch_name, reason)
                        ),
                    };
                    // Only when the agent drifted off its birth branch, which is
                    // deleted too; see `git::RemoveResult::initial_branch_note`.
                    let message = match branches.initial_branch_note(&initial_branch) {
                        Some(note) => format!("{base} {note}"),
                        None => base,
                    };
                    // A surviving branch is a leftover the user has to act on,
                    // so it is a warning rather than the ordinary info line.
                    if branches.branch.refused_reason().is_some()
                        || branches
                            .initial_branch
                            .as_ref()
                            .and_then(|b| b.refused_reason())
                            .is_some()
                    {
                        dux_core::engine::Final::warning(message)
                    } else {
                        dux_core::engine::Final::info(message)
                    }
                }
                TuiDeleteOutcome::SucceededGone {
                    our_busy_still_showing,
                } => {
                    if *our_busy_still_showing {
                        dux_core::engine::Final::info("Worktree removal finished.")
                    } else {
                        dux_core::engine::Final::clear()
                    }
                }
                TuiDeleteOutcome::FailedNamed { message } => dux_core::engine::Final::error(
                    format!("Worktree delete failed for {provider} agent \"{name}\": {message}"),
                ),
                TuiDeleteOutcome::FailedBare { message } => {
                    dux_core::engine::Final::error(format!("Worktree delete failed: {message}"))
                }
            },
        )
    }

    pub(crate) fn begin_delete_session(&mut self, session_id: &str, delete_worktree: bool) {
        match self.engine.apply(Command::BeginDeleteSession {
            session_id: session_id.to_string(),
            delete_worktree,
        }) {
            Ok(reaction) => self.apply_reaction(reaction),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    /// Remove all local bookkeeping for a session whose git side has already
    /// been handled (or does not need handling). Idempotent — if the session
    /// is no longer present this is a no-op, which matters for the async path
    /// where the user may have deleted the project before the worker replies.
    ///
    /// `removal` records what happened to the worktree; it drives the success
    /// message variant.
    /// `update_status` controls whether the method writes a success message
    /// to the status line. The async worker handler passes `false` when the
    /// status line has already been overwritten by an unrelated operation
    /// (push, pull, etc.) to avoid clobbering it. Synchronous callers and
    /// the handler's "our Busy is still showing" path pass `true`.
    pub(crate) fn finish_delete_session(
        &mut self,
        session_id: &str,
        removal: WorktreeRemoval,
        update_status: bool,
    ) -> Result<()> {
        let reaction = self.engine.apply(Command::FinishDeleteSession {
            session_id: session_id.to_string(),
            removal,
            update_status,
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(super) fn apply_finish_delete_session_outcome(
        &mut self,
        session_id: &str,
        outcome: FinishDeleteSessionOutcome,
        removal: WorktreeRemoval,
        update_status: bool,
    ) {
        let FinishDeleteSessionOutcome {
            session,
            project,
            other_sessions_on_worktree: _,
            // No longer needed: the flat list re-clamps the cursor after a delete
            // instead of falling back to a project header.
            project_still_has_sessions: _,
        } = outcome;

        // View-side cleanup the engine couldn't do.
        self.engine.pty_activity.remove(session_id);
        self.engine.pty_input.remove(session_id);
        self.engine.pty_pointer.remove(session_id);
        self.clear_companion_terminals_for_session(session_id);
        self.clear_focused_tab_for_session(session_id);

        // Derived view state. In the flat list, the deleted session is already
        // gone from `engine.sessions`, so the row that slid into the freed slot
        // now sits at the SAME display index. Keep the cursor where it is and let
        // `rebuild_left_items` (which calls `ensure_selectable_left_item`)
        // re-clamp it to a selectable row. The old `saturating_sub(1)` was
        // leftover nested-model logic that double-adjusted and jumped the cursor
        // up one row past the survivor.
        self.rebuild_left_items();
        self.ensure_selectable_left_item();
        self.reload_changed_files();

        if update_status {
            // The branch identity these lines name, when there is one. A
            // standalone agent takes the `NothingToRemove` arm below, which is
            // the only one that reaches it, so the empty fallbacks here are
            // unreachable placeholders rather than values any sentence renders.
            let branch_name = session.branch_name().unwrap_or_default().to_string();
            let initial_branch = session.initial_branch().unwrap_or_default().to_string();
            match removal {
                // A standalone agent: dux's record is gone and the user's
                // folder is exactly as it was. Said out loud, because "Deleted
                // agent X." on its own reads as though something on disk went
                // with it.
                WorktreeRemoval::NothingToRemove { folder_label } => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Its folder \"{folder_label}\" was left untouched: \
                         dux never creates, moves or removes a standalone agent's folder.",
                        session.provider.as_str(),
                        session.display_label(),
                    ));
                }
                WorktreeRemoval::SkippedForSiblings => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved because other sessions still use it.",
                        session.provider.as_str(),
                        branch_name,
                    ));
                }
                WorktreeRemoval::PreservedShared => {
                    self.set_info(format!(
                        "Deleted {} session for agent \"{}\". Worktree preserved for remaining sessions.",
                        session.provider.as_str(),
                        branch_name,
                    ));
                }
                WorktreeRemoval::PreservedOrphan => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved at {}.",
                        session.provider.as_str(),
                        branch_name,
                        session.directory(),
                    ));
                }
                // The worktree went and the branches stayed: they were not
                // dux's to delete. Every kept branch is named with its own
                // reason, and the line says how to remove one by hand.
                WorktreeRemoval::Performed {
                    branches: dux_core::engine::RemovedBranches::Kept(provenance),
                } => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\" and removed its worktree. {}",
                        session.provider.as_str(),
                        branch_name,
                        provenance.kept_branches_note(&branch_name, &initial_branch),
                    ));
                }
                WorktreeRemoval::Performed {
                    branches: dux_core::engine::RemovedBranches::Deleted(branches),
                } => {
                    let mut message = match &branches.branch {
                        dux_core::git::BranchDeletion::Deleted => {
                            let project_name = project
                                .as_ref()
                                .map(|p| p.name.as_str())
                                .unwrap_or("<unknown>");
                            format!(
                                "Deleted {} agent from project \"{}\" with branch \"{}\".",
                                session.provider.as_str(),
                                project_name,
                                branch_name,
                            )
                        }
                        dux_core::git::BranchDeletion::AlreadyGone => format!(
                            "Deleted agent (branch \"{}\" was already removed).",
                            branch_name,
                        ),
                        // Refused means the branch SURVIVED, which is the
                        // opposite of what this line used to claim.
                        dux_core::git::BranchDeletion::Refused { reason } => format!(
                            "Deleted agent, but its branch \"{}\" is still there. {}",
                            branch_name,
                            dux_core::git::branch_refusal_note(&branch_name, reason),
                        ),
                    };
                    // Only when the agent DRIFTED off the branch it was born on:
                    // that second branch is deleted too and the line must say so
                    // rather than leaving the user to discover it.
                    if let Some(note) = branches.initial_branch_note(&initial_branch) {
                        message.push(' ');
                        message.push_str(&note);
                    }
                    let refused = branches.branch.refused_reason().is_some()
                        || branches
                            .initial_branch
                            .as_ref()
                            .and_then(|b| b.refused_reason())
                            .is_some();
                    if refused {
                        self.set_warning(message);
                    } else {
                        self.set_info(message);
                    }
                }
            }
        }
    }

    pub(crate) fn confirm_delete_selected_terminal(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some((terminal_id, terminal)) = items.get(self.selected_terminal_index) else {
            self.set_error("Select a terminal first.");
            return Ok(());
        };
        self.prompt = PromptState::ConfirmDeleteTerminal {
            terminal_id: (*terminal_id).clone(),
            terminal_label: terminal.label.clone(),
            foreground_cmd: terminal.foreground_cmd.clone(),
            focus: ConfirmFocus::Cancel, // Cancel is default
        };
        Ok(())
    }

    pub(crate) fn do_delete_terminal(&mut self, terminal_id: &str) {
        let reaction = match self.engine.apply(Command::DeleteTerminal {
            terminal_id: terminal_id.to_string(),
        }) {
            Ok(r) => r,
            Err(e) => {
                self.set_error(format!("{e:#}"));
                return;
            }
        };
        self.apply_reaction(reaction);
    }

    fn change_agent_provider_options(
        &self,
        session: &AgentSession,
    ) -> Vec<ChangeAgentProviderOption> {
        self.engine
            .config
            .providers
            .commands
            .keys()
            .map(|name| {
                let provider = ProviderKind::new(name.clone());
                let cfg = provider_config(&self.engine.config, &provider);
                let supports_resume = cfg.supports_session_resume();
                let resume_available = supports_resume && session.has_started_provider(&provider);
                ChangeAgentProviderOption {
                    is_current: provider == session.provider,
                    provider,
                    supports_resume,
                    resume_available,
                }
            })
            .collect()
    }

    pub(crate) fn open_change_agent_provider_prompt(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        if self.engine.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        let tab_id = self.focused_tab_id(&session.id);
        self.prompt = PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
            session_id: session.id.clone(),
            tab_id,
            session_label: self.session_label(&session),
            worktree_path: session.directory().to_string(),
            options: self.change_agent_provider_options(&session),
            selected: 0,
            mode: ChangeAgentProviderMode::Retarget,
        });
        self.set_info(
            "Choose a provider for this worktree. The change takes effect on the next launch; dux resumes each provider's prior session on this worktree when available.",
        );
        Ok(())
    }

    pub(crate) fn apply_change_agent_provider(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeAgentProvider(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a provider first.");
            return Ok(());
        };
        let Some(session_index) = self
            .engine
            .sessions
            .iter()
            .position(|session| session.id == prompt.session_id)
        else {
            self.prompt = PromptState::None;
            self.set_error("The selected agent is no longer available.");
            return Ok(());
        };

        if prompt.mode == ChangeAgentProviderMode::NewTab {
            self.prompt = PromptState::None;
            let session_id = self.engine.sessions[session_index].id.clone();
            self.spawn_tab_with_provider(&session_id, selected.provider);
            return Ok(());
        }

        if selected.is_current {
            self.prompt = PromptState::None;
            self.set_info(format!(
                "Agent \"{}\" already uses {}. Pick another provider to swap.",
                prompt.session_label,
                selected.provider.as_str(),
            ));
            return Ok(());
        }

        self.prompt = PromptState::None;

        let session_id = self.engine.sessions[session_index].id.clone();
        // Retarget the focused tab (Main delegates to the session-level change).
        let outcome = self.engine.change_tab_provider(
            &session_id,
            &prompt.tab_id,
            selected.provider.clone(),
        )?;
        self.rebuild_left_items();

        let reconnect_key = self.bindings.label_for(Action::ReconnectAgent);
        if outcome.running {
            self.set_warning(format!(
                "Worktree \"{}\" is set to {}, but the {} agent is still running. Exit it and press {} to relaunch with {}.",
                prompt.session_label,
                selected.provider.as_str(),
                outcome.previous.as_str(),
                reconnect_key,
                selected.provider.as_str(),
            ));
        } else {
            let resume_note = if outcome.resume_available {
                " dux will resume its prior session on this worktree."
            } else {
                " This provider hasn't run on this worktree yet, so it'll start a fresh session."
            };
            self.set_info(format!(
                "Worktree \"{}\" will use {} next launch. Press {} to start it.{}",
                prompt.session_label,
                selected.provider.as_str(),
                reconnect_key,
                resume_note,
            ));
        }
        Ok(())
    }

    fn change_default_provider_options(&self) -> Vec<ChangeDefaultProviderOption> {
        let current = self.engine.config.default_provider();
        self.engine
            .config
            .providers
            .commands
            .keys()
            .map(|name| {
                let provider = ProviderKind::new(name.clone());
                ChangeDefaultProviderOption {
                    is_current: provider == current,
                    provider,
                }
            })
            .collect()
    }

    fn change_project_default_provider_options(
        &self,
        project_id: &str,
    ) -> Vec<ChangeProjectDefaultProviderOption> {
        let global_default = self.engine.config.default_provider();
        let explicit = self.engine.project_explicit_default_provider(project_id);
        let mut options = vec![ChangeProjectDefaultProviderOption {
            provider: None,
            is_current: explicit.is_none(),
        }];
        options.extend(self.engine.config.providers.commands.keys().map(|name| {
            let provider = ProviderKind::new(name.clone());
            ChangeProjectDefaultProviderOption {
                is_current: explicit.as_ref() == Some(&provider),
                provider: Some(provider),
            }
        }));
        if explicit.is_none()
            && !options
                .iter()
                .any(|option| option.provider.as_ref() == Some(&global_default))
        {
            options.push(ChangeProjectDefaultProviderOption {
                provider: Some(global_default),
                is_current: false,
            });
        }
        options
    }

    pub(crate) fn open_change_default_provider_prompt(&mut self) -> Result<()> {
        if self.engine.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }
        let options = self.change_default_provider_options();
        let selected = options
            .iter()
            .position(|option| option.is_current)
            .unwrap_or(0);
        let current = self.engine.config.default_provider();
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeDefaultProvider(ChangeDefaultProviderPrompt {
            current,
            options,
            selected,
        });
        self.set_info(
            "Choose the global default provider for newly created agent sessions. Projects with an explicit project provider keep their override, and existing agents keep their current provider.",
        );
        Ok(())
    }

    pub(crate) fn open_change_project_default_provider_prompt(&mut self) -> Result<()> {
        if self.engine.config.providers.commands.is_empty() {
            self.set_error("No providers are configured.");
            return Ok(());
        }
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        let options = self.change_project_default_provider_options(&project.id);
        let selected = options
            .iter()
            .position(|option| option.is_current)
            .unwrap_or(0);
        let global_default = self.engine.config.default_provider();
        let inherits_global_default = !self
            .engine
            .project_uses_explicit_default_provider(&project.id);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt =
            PromptState::ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt {
                project_id: project.id,
                project_name: project.name,
                current: project.default_provider,
                global_default,
                inherits_global_default,
                options,
                selected,
            });
        self.set_info(
            "Choose the selected project's default provider for future agents. Choose \"inherit global default\" to remove a project-specific override. Existing agents keep their current provider.",
        );
        Ok(())
    }

    pub(crate) fn apply_change_default_provider(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeDefaultProvider(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a provider first.");
            return Ok(());
        };
        self.prompt = PromptState::None;
        if selected.is_current {
            self.set_info(format!(
                "{} is already the global default provider. Pick a different one to change it.",
                selected.provider.as_str(),
            ));
            return Ok(());
        }
        let previous = self.engine.config.defaults.provider.clone();
        self.engine.config.defaults.provider = selected.provider.as_str().to_string();
        if let Err(err) = self
            .engine
            .config_writer
            .save_eager(self.engine.config.clone())
        {
            self.engine.config.defaults.provider = previous;
            self.set_error(format!(
                "Couldn't persist the global default provider change: {err}"
            ));
            return Ok(());
        }
        self.engine.refresh_project_defaults();
        self.rebuild_left_items();
        self.set_info(format!(
            "Global default provider changed to {}. New agents in projects without a project-specific override will use it; existing agents keep their current provider. Use \"change-project-default-provider\" to override one project or \"change-agent-provider\" to switch an existing worktree.",
            selected.provider.as_str(),
        ));
        Ok(())
    }

    pub(crate) fn apply_change_project_default_provider(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeProjectDefaultProvider(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a provider first.");
            return Ok(());
        };
        self.prompt = PromptState::None;
        if selected.is_current {
            let message = match selected.provider {
                Some(provider) => format!(
                    "{} is already the project provider for \"{}\". Pick a different option to change it.",
                    provider.as_str(),
                    prompt.project_name,
                ),
                None => format!(
                    "\"{}\" is already inheriting the global default provider ({}).",
                    prompt.project_name,
                    prompt.global_default.as_str(),
                ),
            };
            self.set_info(message);
            return Ok(());
        }

        if !self
            .engine
            .projects
            .iter()
            .any(|project| project.id == prompt.project_id)
        {
            self.set_error(format!(
                "Could not find project \"{}\".",
                prompt.project_name
            ));
            return Ok(());
        }

        // The final is decided in `apply_project_persistence_outcome` (the
        // post-worker config write is fallible). Declare all three outcomes here
        // on a HandlerStatusOp; the success text matches the handler's branch on
        // `provider`/`global_default` computed at dispatch.
        let project_name = prompt.project_name.clone();
        let global_default = prompt.global_default.clone();
        let provider = selected.provider.clone();
        let success_message = match &provider {
            Some(provider) => format!(
                "Project provider for \"{}\" changed to {}. Future agents in this project will use it; existing agents keep their current provider.",
                project_name,
                provider.as_str(),
            ),
            None => format!(
                "\"{}\" now inherits the global default provider ({}). Future agents in this project will use it; existing agents keep their current provider.",
                project_name,
                global_default.as_str(),
            ),
        };
        let db_fail_name = project_name.clone();
        let config_fail_name = project_name.clone();
        let op = dux_core::engine::status_op(format!(
            "Saving provider preference for project \"{project_name}\"..."
        ))
        .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
            PersistFinalOutcome::Saved => dux_core::engine::Final::info(success_message.clone()),
            PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                "Could not save the provider change for project \"{db_fail_name}\": {error}"
            )),
            PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                "Provider preference saved to the database for \"{config_fail_name}\", but config.toml could not be updated: {err}"
            )),
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_persist_ops.insert(op_id.clone(), op);
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::UpdateDefaultProvider {
                project_id: prompt.project_id,
                project_name: prompt.project_name.clone(),
                provider: selected.provider,
                global_default: prompt.global_default,
            }),
            status_op_id: Some(op_id),
        })?;
        self.apply_reaction(reaction);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn toggle_project_auto_reopen_agents(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        let enabled = self.engine.project_allows_auto_reopen(&project.id);
        let auto_reopen_agents = if enabled { Some(false) } else { None };
        let project_name = project.name.clone();
        // Mirror the handler's success branch: it derives enabled/disabled from
        // `auto_reopen_agents.unwrap_or(true)`.
        let new_enabled = auto_reopen_agents.unwrap_or(true);
        let success_name = project_name.clone();
        let db_fail_name = project_name.clone();
        let config_fail_name = project_name.clone();
        let op = dux_core::engine::status_op(format!(
            "Saving auto-reopen preference for project \"{project_name}\"..."
        ))
        .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
            PersistFinalOutcome::Saved => dux_core::engine::Final::info(format!(
                "Startup auto-reopen {} for project \"{}\".",
                if new_enabled { "enabled" } else { "disabled" },
                success_name,
            )),
            PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                "Could not save the auto-reopen change for project \"{db_fail_name}\": {error}"
            )),
            PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                "Auto-reopen preference saved to the database for \"{config_fail_name}\", but config.toml could not be updated: {err}"
            )),
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_persist_ops.insert(op_id.clone(), op);
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::UpdateAutoReopen {
                project_id: project.id.clone(),
                project_name: project.name.clone(),
                auto_reopen_agents,
            }),
            status_op_id: Some(op_id),
        })?;
        self.apply_reaction(reaction);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn toggle_agent_auto_reopen(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        let new_enabled = !session.auto_reopen_enabled;
        let reaction = self.engine.apply(Command::ToggleAgentAutoReopen {
            branch_name: session.display_label(),
            session_id: session.id,
            new_enabled,
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(crate) fn open_configure_startup_command(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ConfigureStartupCommand {
            project_id: project.id,
            project_name: project.name.clone(),
            input: TextInput::with_text(project.startup_command.unwrap_or_default())
                .with_multiline(6)
                .with_placeholder("Enter startup command..."),
            focus: ConfigureFieldFocus::default(),
        };
        self.input_target = InputTarget::None;
        self.set_info("Enter a startup command for this project. Empty clears it.");
        Ok(())
    }

    pub(crate) fn apply_configure_startup_command(&mut self) -> Result<()> {
        let (project_id, project_name, command) = match &self.prompt {
            PromptState::ConfigureStartupCommand {
                project_id,
                project_name,
                input,
                ..
            } => (
                project_id.clone(),
                project_name.clone(),
                input.text.trim().to_string(),
            ),
            _ => return Ok(()),
        };
        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        if !self
            .engine
            .projects
            .iter()
            .any(|project| project.id == project_id)
        {
            self.set_error(format!("Could not find project \"{project_name}\"."));
            return Ok(());
        }
        let startup_command = (!command.is_empty()).then_some(command);
        let success_command = startup_command.clone();
        let success_name = project_name.clone();
        let db_fail_name = project_name.clone();
        let config_fail_name = project_name.clone();
        let op = dux_core::engine::status_op(format!(
            "Saving startup command for project \"{project_name}\"..."
        ))
        .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
            PersistFinalOutcome::Saved => match &success_command {
                Some(command) => dux_core::engine::Final::info(format!(
                    "Startup command for project \"{success_name}\" set to: {command}"
                )),
                None => dux_core::engine::Final::info(format!(
                    "Startup command cleared for project \"{success_name}\"."
                )),
            },
            PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                "Could not save the startup command for project \"{db_fail_name}\": {error}"
            )),
            PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                "Startup command saved to the database for \"{config_fail_name}\", but config.toml could not be updated: {err}"
            )),
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_persist_ops.insert(op_id.clone(), op);
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::UpdateStartupCommand {
                project_id,
                project_name: project_name.clone(),
                startup_command,
            }),
            status_op_id: Some(op_id),
        })?;
        self.apply_reaction(reaction);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn open_configure_project_env(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ConfigureProjectEnv {
            project_id: project.id,
            project_name: project.name.clone(),
            input: TextInput::with_text(crate::config::project_env_to_lines(&project.env))
                .with_multiline(8)
                .with_placeholder("KEY=value"),
            focus: ConfigureFieldFocus::default(),
        };
        self.set_info("Enter one environment variable per line as KEY=value. Empty clears them.");
        Ok(())
    }

    pub(crate) fn open_configure_global_env(&mut self) -> Result<()> {
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ConfigureGlobalEnv {
            project_name: "All projects".to_string(),
            input: TextInput::with_text(crate::config::project_env_to_lines(
                &self.engine.config.env,
            ))
            .with_multiline(8)
            .with_placeholder("KEY=value"),
            focus: ConfigureFieldFocus::default(),
        };
        self.set_info("Enter global environment variables as KEY=value. Empty clears them.");
        Ok(())
    }

    pub(crate) fn apply_configure_global_env(&mut self) -> Result<()> {
        let env = match &self.prompt {
            PromptState::ConfigureGlobalEnv { input, .. } => {
                match crate::config::parse_project_env_lines(&input.text) {
                    Ok(env) => env,
                    Err(err) => {
                        self.set_error(format!(
                            "Global environment variables are invalid: {err:#}"
                        ));
                        return Ok(());
                    }
                }
            }
            _ => return Ok(()),
        };
        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        // PersistGlobalEnv now eager-saves and returns a FINAL status synchronously
        // (success or rollback error); surface that and do NOT set a trailing Busy,
        // which would never clear (the work already completed).
        let reaction = self.engine.apply(Command::PersistGlobalEnv { env })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(crate) fn apply_configure_project_env(&mut self) -> Result<()> {
        let (project_id, project_name, env) = match &self.prompt {
            PromptState::ConfigureProjectEnv {
                project_id,
                project_name,
                input,
                ..
            } => {
                let env = match crate::config::parse_project_env_lines(&input.text) {
                    Ok(env) => env,
                    Err(err) => {
                        self.set_error(format!(
                            "Environment variables for project \"{project_name}\" are invalid: {err:#}"
                        ));
                        return Ok(());
                    }
                };
                (project_id.clone(), project_name.clone(), env)
            }
            _ => return Ok(()),
        };
        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        if !self
            .engine
            .projects
            .iter()
            .any(|project| project.id == project_id)
        {
            self.set_error(format!("Could not find project \"{project_name}\"."));
            return Ok(());
        }
        let env_count = env.len();
        let success_name = project_name.clone();
        let db_fail_name = project_name.clone();
        let config_fail_name = project_name.clone();
        let op = dux_core::engine::status_op(format!(
            "Saving environment variables for project \"{project_name}\"..."
        ))
        .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
            PersistFinalOutcome::Saved => {
                if env_count == 0 {
                    dux_core::engine::Final::info(format!(
                        "Environment variables cleared for project \"{success_name}\"."
                    ))
                } else {
                    dux_core::engine::Final::info(format!(
                        "Saved {env_count} environment variable(s) for project \"{success_name}\". New agents and terminals will receive them.",
                    ))
                }
            }
            PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                "Could not save environment variables for project \"{db_fail_name}\": {error}"
            )),
            PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                "Environment variables saved to the database for \"{config_fail_name}\", but config.toml could not be updated: {err}"
            )),
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_persist_ops.insert(op_id.clone(), op);
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::UpdateEnv {
                project_id,
                project_name: project_name.clone(),
                env,
            }),
            status_op_id: Some(op_id),
        })?;
        self.apply_reaction(reaction);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn rerun_startup_command_on_agent(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        // A startup command provisions a worktree for a project, so a
        // standalone agent has neither one to run nor a place to run it. The
        // refusal names the shape of the thing rather than reporting a missing
        // project record, which would suggest something broke.
        let managed = match self.engine.branch_git_workspace(
            &session.id,
            "run a startup command for",
            "A startup command provisions a new worktree, and this agent runs in a folder that already exists.",
        ) {
            Ok(managed) => managed.clone(),
            Err(err) => {
                self.set_error(err.to_string());
                return Ok(());
            }
        };
        let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|project| project.id == managed.project_id)
            .cloned()
        else {
            self.set_error("Could not find the selected agent's project.");
            return Ok(());
        };
        let Some(command) = project
            .startup_command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_string)
        else {
            self.set_error(format!(
                "Project \"{}\" does not have a startup command.",
                project.name
            ));
            return Ok(());
        };
        let paths = self.engine.paths.clone();
        let tx = self.engine.worker_tx.clone();
        let branch = managed.branch_name.clone();
        let terminal = self.engine.config.startup_command_terminal.clone();
        let env = crate::config::resolve_agent_env(&self.engine.config.env, &project.env)
            .unwrap_or_default();
        // Declare the loading→final states together. The success message needs
        // the palette keybinding label (render context only the main thread has);
        // resolve it HERE and bake it into the op's outcomes. The status rides a
        // separate StatusOpCompleted event from the worker.
        let palette_key = self.bindings.label_for(Action::OpenPalette);
        let success_name = project.name.clone();
        let failure_name = project.name.clone();
        let op = dux_core::engine::status_op(format!(
            "Rerunning startup command for agent \"{branch}\"..."
        ))
        .on_success(move |_: &()| {
            dux_core::engine::Final::info(format!(
                "Startup command completed for project \"{success_name}\". Press {palette_key} and run read-startup-command-logs to view the latest log.",
            ))
        })
        .on_failure(move |err: &String| {
            dux_core::engine::Final::error(format!(
                "Startup command failed for project \"{failure_name}\": {err}. Run read-startup-command-logs for details.",
            ))
        });
        let pending = op.pending_status();
        std::thread::spawn(move || {
            let result = crate::startup::run_startup_command(
                &paths,
                crate::startup::StartupCommandRun {
                    project,
                    session,
                    managed,
                    command,
                    terminal,
                    env,
                },
            );
            let resolved = op.resolve(&result.status);
            let _ = tx.send(WorkerEvent::StatusOpCompleted { resolved });
        });
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    pub(crate) fn open_startup_command_logs(&mut self) -> Result<()> {
        // A standalone agent has no project and so no startup-command logs;
        // the scope falls through to the selected project, or to nothing.
        let selected_agent_scope = self
            .selected_session()
            .cloned()
            .filter(|session| session.project_id().is_some());
        let (scope_label, scope) = if let Some(session) = selected_agent_scope {
            let project_name = self.engine.project_name_for_session(&session);
            let project_id = session
                .project_id()
                .expect("filtered to agents with a project")
                .to_string();
            (
                format!(
                    "agent \"{}\" in project \"{}\"",
                    session.display_label(),
                    project_name
                ),
                crate::startup::StartupCommandLogScope::Agent {
                    project_id,
                    session_id: session.id,
                },
            )
        } else if let Some(project) = self.selected_project().cloned() {
            (
                format!("project \"{}\"", project.name),
                crate::startup::StartupCommandLogScope::Project {
                    project_id: project.id,
                },
            )
        } else {
            self.set_error("Select an agent or project first.");
            return Ok(());
        };

        self.spawn_startup_command_log_load(scope_label, scope);
        Ok(())
    }

    /// Move the picker's selection onto `selected` and load that run's output.
    ///
    /// The selection moves NOW and the body follows: the read is a worker hop
    /// (see [`App::spawn_startup_command_log_content_load`]), so walking the
    /// list never blocks the UI thread on a log that could be megabytes of
    /// captured `npm install` output on a cold or networked filesystem.
    ///
    /// Re-selecting the row that is already selected is a no-op, so a click on
    /// the current row, or a filter keystroke whose first match does not move,
    /// does not re-read the file.
    pub(crate) fn select_startup_command_log(&mut self, selected: usize) {
        let Some((path, display_name, count, already_selected)) = (match &self.prompt {
            PromptState::StartupCommandLogs(prompt) => prompt.entries.get(selected).map(|entry| {
                (
                    entry.path.clone(),
                    entry.display_name.clone(),
                    prompt.entries.len(),
                    prompt.selected == selected,
                )
            }),
            _ => None,
        }) else {
            return;
        };
        if already_selected {
            return;
        }
        if let PromptState::StartupCommandLogs(prompt) = &mut self.prompt {
            prompt.selected = selected.min(count.saturating_sub(1));
            prompt.content = format!("Reading {display_name}...");
            prompt.scroll_offset = 0;
        }
        self.startup_log_selection = None;
        self.spawn_startup_command_log_content_load(path, display_name);
    }

    /// Apply a run body that finished reading off-thread.
    ///
    /// Dropped unless `path` is still the selected run: a fast walk down the
    /// list has several reads in flight at once and they can land out of order,
    /// so the selected path is the correlation handle.
    pub(crate) fn apply_startup_command_log_content(
        &mut self,
        path: &Path,
        result: Result<String, String>,
    ) {
        let PromptState::StartupCommandLogs(prompt) = &mut self.prompt else {
            return;
        };
        if prompt
            .entries
            .get(prompt.selected)
            .map(|e| e.path.as_path())
            != Some(path)
        {
            return;
        }
        prompt.content = match result {
            Ok(content) => content,
            Err(err) => format!("Could not read {}: {err}", path.display()),
        };
        prompt.scroll_offset = 0;
    }

    /// Promote the picker's selected run to the fullscreen viewer.
    ///
    /// No I/O: the body the picker is already showing is the body the viewer
    /// gets, so this is instant and cannot disagree with what was on screen.
    ///
    /// The picker itself rides along on the viewer as its `return_to` ticket,
    /// so closing the viewer restores this exact run list rather than dropping
    /// the user out of the journey (see [`App::close_top_overlay`]).
    pub(crate) fn promote_startup_command_log_to_fullscreen(&mut self) {
        let PromptState::StartupCommandLogs(prompt) = &self.prompt else {
            return;
        };
        let Some(entry) = prompt.entries.get(prompt.selected) else {
            self.set_error("No startup command log is selected.");
            return;
        };
        let viewer = StartupLogViewer {
            scope_label: prompt.scope_label.clone(),
            path: Some(entry.path.clone()),
            display_name: entry.display_name.clone(),
            content: prompt.content.clone(),
            scroll_offset: 0,
            // Inherit the width the promoting picker was measured at, so a
            // promotion that keeps the same body width keeps its scroll.
            wrap_width: prompt.wrap_width,
            search: TextInput::new(),
            searching: false,
            return_to: Some(Box::new(prompt.clone())),
        };
        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
        self.startup_log_selection = None;
        self.terminal_selection = None;
        self.fullscreen_overlay = FullscreenOverlay::StartupLog;
        self.startup_log_viewer = Some(viewer);
    }

    pub(crate) fn startup_command_log_filtered_indices(
        prompt: &StartupCommandLogPrompt,
    ) -> Vec<usize> {
        let query = prompt.filter.text.trim().to_lowercase();
        prompt
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                (query.is_empty() || entry.display_name.to_lowercase().contains(&query))
                    .then_some(index)
            })
            .collect()
    }

    pub(crate) fn startup_command_log_selected_visual_index(
        prompt: &StartupCommandLogPrompt,
        visible_indices: &[usize],
    ) -> Option<usize> {
        visible_indices
            .iter()
            .position(|index| *index == prompt.selected)
    }

    pub(crate) fn select_startup_command_log_visual_index(&mut self, visual_index: usize) {
        let Some(actual_index) = (match &self.prompt {
            PromptState::StartupCommandLogs(prompt) => {
                Self::startup_command_log_filtered_indices(prompt)
                    .get(visual_index)
                    .copied()
            }
            _ => None,
        }) else {
            return;
        };
        self.select_startup_command_log(actual_index);
    }

    /// The log file the "open in the OS" actions act on.
    ///
    /// Both surfaces bind those actions, so both have to be asked, and the
    /// PICKER wins when it is open because it is the one on top. Resolving only
    /// from the viewer was a real bug: the picker reported "No startup command
    /// log is selected." with a row plainly highlighted. It went unnoticed
    /// because nothing outside tests could open the picker; opening it on the
    /// read-logs journey is what makes the path reachable.
    pub(crate) fn selected_startup_command_log_path(&self) -> Option<PathBuf> {
        match &self.prompt {
            PromptState::StartupCommandLogs(prompt) => prompt
                .entries
                .get(prompt.selected)
                .map(|entry| entry.path.clone()),
            _ => self
                .startup_log_viewer
                .as_ref()
                .and_then(|viewer| viewer.path.clone()),
        }
    }

    pub(crate) fn open_selected_startup_command_log(&mut self) {
        let Some(path) = self.selected_startup_command_log_path() else {
            self.set_error("No startup command log is selected.");
            return;
        };
        self.spawn_open_path(path, "startup command log file");
    }

    pub(crate) fn open_selected_startup_command_log_folder(&mut self) {
        let Some(path) = self
            .selected_startup_command_log_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        else {
            self.set_error("No startup command log folder is selected.");
            return;
        };
        self.spawn_open_path(path, "startup command log folder");
    }

    fn spawn_open_path(&mut self, path: PathBuf, target: &'static str) {
        match self.engine.apply(Command::OpenPath {
            path,
            target: target.to_string(),
        }) {
            Ok(reaction) => self.apply_reaction(reaction),
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    fn spawn_startup_command_log_load(
        &mut self,
        scope_label: String,
        scope: crate::startup::StartupCommandLogScope,
    ) {
        let paths = self.engine.paths.clone();
        let tx = self.engine.worker_tx.clone();
        // Declare the loading→final states together. The status rides a separate
        // StatusOpCompleted event; the StartupLogArrived domain event (which opens
        // the overlay) keeps doing only its domain work.
        let success_label = scope_label.clone();
        let failure_label = scope_label.clone();
        let op = dux_core::engine::status_op(format!(
            "Opening startup command logs for {scope_label}..."
        ))
        // Three outcomes, not two: a scope that has simply never run its
        // startup command is a success with nothing to show, and it must say so
        // rather than resolve as "Opened ..." over a surface that never opened.
        .on_success(move |listing: &crate::startup::StartupCommandLogListing| {
            if listing.entries.is_empty() {
                dux_core::engine::Final::info(format!(
                    "No startup command logs recorded for {success_label} yet."
                ))
            } else {
                dux_core::engine::Final::info(format!(
                    "Opened {} startup command log run(s) for {success_label}.",
                    listing.entries.len()
                ))
            }
        })
        .on_failure(move |err: &String| {
            dux_core::engine::Final::error(format!(
                "Could not read startup command logs for {failure_label}: {err}"
            ))
        });
        let pending = op.pending_status();
        std::thread::spawn(move || {
            let result = crate::startup::load_logs_for_scope(&paths, scope)
                .map_err(|err| format!("{err:#}"));
            let resolved = op.resolve(&result);
            let _ = tx.send(WorkerEvent::StatusOpCompleted { resolved });
            // Domain work exists only when there is something to open. The
            // failure AND the nothing-recorded outcomes are fully carried by
            // the StatusOpCompleted above.
            match result {
                Ok(listing) if !listing.entries.is_empty() => {
                    let _ = tx.send(WorkerEvent::StartupCommandLogsLoaded {
                        scope_label,
                        result: Ok(listing),
                    });
                }
                _ => {}
            }
        });
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
    }

    /// Read one already-listed run off-thread, for the picker's preview.
    ///
    /// The final is [`Final::clear`]: the output appearing in the pane IS the
    /// confirmation, and a success line per arrow-key press would be noise on a
    /// status surface that is most-recent-wins. A failure still speaks up.
    fn spawn_startup_command_log_content_load(&mut self, path: PathBuf, display_name: String) {
        let tx = self.engine.worker_tx.clone();
        let failure_name = display_name.clone();
        let op =
            dux_core::engine::status_op(format!("Reading startup command log {display_name}..."))
                .on_success(|_: &String| dux_core::engine::Final::clear())
                .on_failure(move |err: &String| {
                    dux_core::engine::Final::error(format!(
                        "Could not read startup command log {failure_name}: {err}"
                    ))
                });
        let pending = op.pending_status();
        let read_path = path.clone();
        std::thread::spawn(move || {
            let result = crate::startup::read_log(&read_path).map_err(|err| format!("{err:#}"));
            let resolved = op.resolve(&result);
            let _ = tx.send(WorkerEvent::StatusOpCompleted { resolved });
            let _ = tx.send(WorkerEvent::StartupCommandLogContentLoaded { path, result });
        });
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
    }

    pub(crate) fn open_change_theme_prompt(&mut self) -> Result<()> {
        let options = crate::theme::discover_available(&self.engine.paths);
        if options.is_empty() {
            self.set_error("No themes available.");
            return Ok(());
        }
        let current = self.engine.config.ui.theme.clone();
        let selected = options
            .iter()
            .position(|opt| opt.id == current)
            .unwrap_or(0);
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::ChangeTheme(ChangeThemePrompt {
            options,
            selected,
            current,
        });
        self.set_info(
            "Themes preview live as you move. Enter saves the choice; Esc reverts to the previous theme.",
        );
        Ok(())
    }

    /// Live-preview the theme at the prompt's current selection. Called every
    /// time the user moves the cursor in the picker (keyboard or mouse) so
    /// the whole UI repaints with the highlighted theme without having to
    /// commit anything yet. Failures are swallowed — a theme that won't load
    /// just leaves the previously-previewed theme in place; the picker stays
    /// open so the user can pick a different one.
    pub(crate) fn preview_change_theme_selection(&mut self) {
        let id = match &self.prompt {
            PromptState::ChangeTheme(prompt) => prompt
                .options
                .get(prompt.selected)
                .map(|option| option.id.clone()),
            _ => None,
        };
        let Some(id) = id else { return };
        if let Ok(theme) = crate::theme::load(&id, &self.engine.paths) {
            self.theme = theme;
        }
    }

    /// Cancel the theme picker. Reloads the theme that was active when the
    /// picker opened so any live previews are reverted.
    pub(crate) fn cancel_change_theme(&mut self) {
        let original = match &self.prompt {
            PromptState::ChangeTheme(prompt) => Some(prompt.current.clone()),
            _ => None,
        };
        self.prompt = PromptState::None;
        if let Some(original) = original
            && let Ok(theme) = crate::theme::load(&original, &self.engine.paths)
        {
            self.theme = theme;
        }
    }

    pub(crate) fn apply_change_theme(&mut self) -> Result<()> {
        let prompt = match &self.prompt {
            PromptState::ChangeTheme(prompt) => prompt.clone(),
            _ => return Ok(()),
        };
        let Some(selected) = prompt.options.get(prompt.selected).cloned() else {
            self.prompt = PromptState::None;
            self.set_error("Select a theme first.");
            return Ok(());
        };
        self.prompt = PromptState::None;
        if selected.id == prompt.current {
            self.set_info(format!(
                "Theme \"{}\" is already active. Pick a different one to change it.",
                selected.display_name,
            ));
            return Ok(());
        }
        let theme = match crate::theme::load(&selected.id, &self.engine.paths) {
            Ok(theme) => theme,
            Err(err) => {
                self.set_error(format!(
                    "Couldn't load theme \"{}\": {err:#}",
                    selected.display_name
                ));
                return Ok(());
            }
        };
        let previous = self.engine.config.ui.theme.clone();
        self.engine.config.ui.theme = selected.id.clone();
        if let Err(err) = self
            .engine
            .config_writer
            .save_eager(self.engine.config.clone())
        {
            self.engine.config.ui.theme = previous;
            self.set_error(format!(
                "Couldn't persist the theme change: {err}. The new theme is loaded for this session only."
            ));
            // Still apply to the running session — the user explicitly asked
            // for it and we'd rather flash a wrong-color UI than silently
            // ignore the request.
            self.theme = theme;
            return Ok(());
        }
        self.theme = theme;
        self.set_info(format!(
            "Theme changed to \"{}\". Future sessions will use it too.",
            selected.display_name,
        ));
        Ok(())
    }

    pub(crate) fn remove_selected_project(&mut self) -> Result<()> {
        if let Some(project) = self.take_selected_project() {
            // Real project: keep the guard. Removing one that still has agents
            // here would orphan them — use "delete project" to remove agents too.
            let has_sessions = self
                .engine
                .sessions
                .iter()
                .any(|s| s.project_id() == Some(project.id.as_str()));
            if has_sessions {
                self.set_error("Delete all agents in this project first.");
                return Ok(());
            }
            let project_name = project.name.clone();
            let success_name = project_name.clone();
            let db_fail_name = project_name.clone();
            let op = dux_core::engine::status_op(format!(
                "Removing project \"{project_name}\" from workspace..."
            ))
            .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
                PersistFinalOutcome::Saved => dux_core::engine::Final::info(format!(
                    "Removed project \"{success_name}\" from app"
                )),
                PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                    "Could not remove project \"{db_fail_name}\" from the database: {error}"
                )),
                PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                    "Project was removed from the database, but config.toml could not be updated: {err}"
                )),
            });
            let pending = op.pending_status();
            let op_id = op.id().to_string();
            self.pending_persist_ops.insert(op_id.clone(), op);
            let reaction = self.engine.apply(Command::PersistProject {
                action: Box::new(ProjectPersistenceAction::Remove {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                }),
                status_op_id: Some(op_id),
            })?;
            self.apply_reaction(reaction);
            self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
            return Ok(());
        }
        // No real project is selected. If an ORPHANED session is selected (its
        // project record is gone), clear the whole ghost group: Command::RemoveProject
        // cascades the orphaned session records and keeps their worktrees on disk.
        // A STANDALONE agent is not an orphan: it has no project record to
        // have lost, so there is no ghost group to clear for it.
        if let Some(session) = self.selected_session().cloned()
            && let Some(project_id) = session.project_id().map(str::to_string)
        {
            let project_name = dux_core::sidebar::short_project_id(&project_id);
            let reaction = self.engine.apply(Command::RemoveProject {
                project_id,
                project_name,
            })?;
            self.apply_reaction(reaction);
            // The cascade mutates engine.sessions synchronously; refresh the cache
            // (and fix the selection) so render never indexes a stale row.
            self.rebuild_left_items();
            return Ok(());
        }
        self.set_error("Select a project first.");
        Ok(())
    }

    pub(crate) fn delete_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.take_selected_project() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        // The whole delete (guards for in-flight worktree removals / launching
        // tabs, the per-session cascade with worktree removal, and the project
        // record + config removal) is owned by the core `Command::DeleteProject`
        // so the TUI and the web can never disagree on the sequencing. This path
        // is synchronous exactly as before (the cascade runs `git worktree remove`
        // inline), so no async status op is needed.
        logger::info(&format!("deleting project {}", project.path));
        let reaction = self.engine.apply(Command::DeleteProject {
            project_id: project.id.clone(),
            project_name: project.name.clone(),
        })?;
        self.apply_reaction(reaction);
        // The cascade mutated engine.sessions/projects synchronously; refresh the
        // cache (and fix the selection) so render never indexes a stale row.
        self.rebuild_left_items();
        Ok(())
    }

    /// Restart the selected agent with a fresh session, bypassing `--continue`
    /// or equivalent resume args. Works on both active and detached agents.
    /// Routes through the shared `dispatch_reconnect_plan` (`force == true`) so
    /// the guards, teardown, and message are the single-source `reconnect_plan`.
    pub(crate) fn force_reconnect_agent(&mut self) -> Result<()> {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        logger::info(&format!(
            "restarting agent {session_id} with fresh session (no resume args)"
        ));
        self.dispatch_reconnect_plan(&session_id, true, false)
    }

    /// `seek_fullscreen` marks a fullscreen-seeking relaunch (decision 10):
    /// only the fullscreen toggle passes `true`; every other caller lands the
    /// completed launch focused-but-minimized (a relaunch initiated FROM the
    /// fullscreen relaunch screen still lands fullscreen; see
    /// `launch_seeks_fullscreen`).
    pub(crate) fn reconnect_selected_session(&mut self, seek_fullscreen: bool) -> Result<()> {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error("Select a stopped agent first to reconnect.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {session_id}"));
        self.dispatch_reconnect_plan(&session_id, false, seek_fullscreen)
    }

    /// Shared TUI reconnect dispatch: build the single-source
    /// `Engine::reconnect_plan` (guards, the collision-aware resume decision, the
    /// message, and the pre-dispatch mutations all in core) and render each
    /// variant. `reconnect_selected_session` (`force == false`) and
    /// `force_reconnect_agent` (`force == true`) both route here so neither
    /// recomputes the resume decision (which used to be announced via the
    /// collision-blind `should_resume_session` while the dispatch re-gated,
    /// promising a resume that launched fresh).
    fn dispatch_reconnect_plan(
        &mut self,
        session_id: &str,
        force: bool,
        seek_fullscreen: bool,
    ) -> Result<()> {
        let pty_size = self.pty_size_for_launch();
        match self.engine.reconnect_plan(session_id, force, pty_size)? {
            dux_core::engine::ReconnectPlan::AlreadyConnected { message } => {
                self.set_info(message);
            }
            dux_core::engine::ReconnectPlan::WorktreeMissing { message } => {
                self.set_error(message);
            }
            dux_core::engine::ReconnectPlan::Launch {
                mut request,
                busy_message,
                ..
            } => {
                request.wants_fullscreen = self.launch_seeks_fullscreen(seek_fullscreen);
                if self.dispatch_agent_launch(*request) {
                    // Route the busy through a keyed reconnect op so its final
                    // (resolved in the shared launch-ready/failed view handlers)
                    // replaces exactly this spinner instead of most-recent-wins.
                    let op = self.build_reconnect_status_op(busy_message);
                    self.apply_reaction(dux_core::engine::EventReaction::Status(
                        op.pending_status(),
                    ));
                    self.pending_reconnect_ops
                        .insert(session_id.to_string(), op);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn open_diff_for_selected_file(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let Some(file) = self.selected_changed_file() else {
            return Ok(());
        };
        let worktree_path = session.directory().to_string();
        let rel_path = file.path.clone();
        let output = crate::diff::diff_file(
            Path::new(&worktree_path),
            &rel_path,
            &self.theme,
            &self.syntax_cache,
            self.show_diff_line_numbers,
            self.engine.config.ui.diff_tab_width,
        )?;
        self.center_mode = CenterMode::Diff {
            lines: Arc::new(output.lines),
            scroll: 0,
            gutter_width: output.gutter_width,
            worktree_path,
            rel_path,
        };
        self.focus = FocusPane::Center;
        Ok(())
    }

    /// Re-generate the currently displayed diff (e.g. after toggling line numbers).
    pub(crate) fn refresh_current_diff(&mut self) -> Result<()> {
        let (worktree_path, rel_path, scroll) = match &self.center_mode {
            CenterMode::Diff {
                worktree_path,
                rel_path,
                scroll,
                ..
            } => (worktree_path.clone(), rel_path.clone(), *scroll),
            _ => return Ok(()),
        };
        let output = crate::diff::diff_file(
            Path::new(&worktree_path),
            &rel_path,
            &self.theme,
            &self.syntax_cache,
            self.show_diff_line_numbers,
            self.engine.config.ui.diff_tab_width,
        )?;
        self.center_mode = CenterMode::Diff {
            lines: Arc::new(output.lines),
            scroll,
            gutter_width: output.gutter_width,
            worktree_path,
            rel_path,
        };
        Ok(())
    }

    pub(crate) fn copy_selected_path(&mut self) -> Result<()> {
        // Agent selection wins: copy the selected agent's worktree path.
        let agent_path = match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self
                .engine
                .sessions
                .get(*index)
                .map(|s| s.directory().to_string()),
            _ => None,
        };
        // Fall back to a chooser-picked project (agent-less projects have no row
        // to select, so this is how their path is reachable). `take_selected_project`
        // consumes the one-and-done `manage-projects` target.
        let (path, label) = match agent_path {
            Some(p) => (Some(p), "Agent's path copied to clipboard."),
            None => (
                self.take_selected_project().map(|p| p.path),
                "Project's path copied to clipboard.",
            ),
        };
        match path {
            Some(p) => {
                match self.clipboard.copy_text(&p, label, &self.engine.worker_tx) {
                    Ok(pending) => {
                        self.apply_reaction(dux_core::engine::EventReaction::Status(pending))
                    }
                    Err(e) => self.set_error(format!("Copy path failed: {e}")),
                }
                Ok(())
            }
            None => {
                self.set_error("No project or agent selected. Select one from the sidebar first.");
                Ok(())
            }
        }
    }

    pub(crate) fn open_selected_worktree_in_default_editor(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        let editors = editor::detect_installed_editors();
        let Some(selected_editor) =
            editor::preferred_editor(&editors, &self.engine.config.editor.default)
        else {
            self.set_error(
                "No supported editor CLI found on PATH. Install cursor, code, zed, vscodium, or sublime.",
            );
            return Ok(());
        };

        let session_label = self.session_label(&session);
        let configured_default = self.engine.config.editor.default.trim().to_string();
        self.open_worktree_in_editor(session.directory(), &session_label, &selected_editor)?;

        if !configured_default.is_empty()
            && !editor::matches_configured_editor(&selected_editor, &configured_default)
        {
            self.set_info(format!(
                "Opened agent \"{session_label}\" in {} via {} (configured default \"{}\" was not found on PATH).",
                selected_editor.label, selected_editor.command, configured_default
            ));
        }

        Ok(())
    }

    pub(crate) fn open_worktree_editor_picker(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        let editors = editor::detect_installed_editors();
        if editors.is_empty() {
            self.set_error(
                "No supported editor CLI found on PATH. Install cursor, code, zed, vscodium, or sublime.",
            );
            return Ok(());
        }

        let selected = editor::preferred_editor(&editors, &self.engine.config.editor.default)
            .and_then(|preferred| {
                editors
                    .iter()
                    .position(|editor| editor.command == preferred.command)
            })
            .unwrap_or(0);
        let session_label = self.session_label(&session);
        self.prompt = PromptState::PickEditor {
            session_label,
            worktree_path: session.directory().to_string(),
            editors,
            selected,
        };
        self.set_info("Choose an editor and press Enter to open the selected worktree.");
        Ok(())
    }

    pub(crate) fn open_worktree_in_editor(
        &mut self,
        worktree_path: &str,
        session_label: &str,
        editor_choice: &editor::DetectedEditor,
    ) -> Result<()> {
        editor::launch_editor(editor_choice, Path::new(worktree_path))?;
        self.set_info(format!(
            "Opened agent \"{session_label}\" in {} via {}.",
            editor_choice.label, editor_choice.command
        ));
        Ok(())
    }

    pub(crate) fn current_pr_info(&self) -> Option<&crate::model::PrInfo> {
        self.selected_session()
            .and_then(|session| self.engine.pr_statuses.get(&session.id))
    }

    pub(crate) fn current_pr_url(&self) -> Option<&str> {
        self.current_pr_info().map(|pr| pr.url.as_str())
    }

    pub(crate) fn open_current_pr_in_browser(&mut self) -> Result<()> {
        let Some(pr) = self.current_pr_info().cloned() else {
            self.set_error("No pull request is known for the selected agent yet.");
            return Ok(());
        };

        let url = self.current_pr_url().unwrap_or(pr.url.as_str()).to_string();
        browser::open_url(&url)?;
        self.set_info(format!(
            "Opened PR {}#{} in the default browser.",
            pr.owner_repo, pr.number
        ));
        Ok(())
    }

    /// `attach-pull-request`: open the reference field for the selected agent.
    /// The session fixes the project, so unlike the create-from-PR modal there
    /// is no project to choose and the field is the only control.
    pub(crate) fn open_attach_pull_request_prompt(&mut self) -> Result<()> {
        if !self.github_pr_agent_command_available() {
            self.set_error(
                "Attaching a pull request requires GitHub integration and an authenticated gh CLI.",
            );
            return Ok(());
        }
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("No agent session selected. Select an agent to attach a pull request.");
            return Ok(());
        };
        // The body names the PR currently shown so overriding it is explicit.
        let current_pr = self.engine.pr_statuses.get(&session.id).map(|pr| {
            let overridden = self.engine.pr_overrides.contains_key(&session.id);
            format!(
                "#{} ({}) {}{}",
                pr.number,
                super::pr_state_word(&pr.state),
                pr.title,
                if overridden {
                    " (manually attached)"
                } else {
                    ""
                }
            )
        });
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::AttachPullRequestInput {
            session_id: session.id,
            current_pr,
            input: TextInput::new(),
        };
        Ok(())
    }

    /// `detach-pull-request`: the selected agent has no pull request, as of
    /// now. Drops a pin if there is one, clears the badge, and stops
    /// autodetection for the agent until it is attached by hand or detection
    /// is resumed. Synchronous and reversible both ways, so no modal and no
    /// confirmation.
    pub(crate) fn detach_pull_request(&mut self) -> Result<()> {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error(
                "No agent session selected. Select an agent to detach its pull request.",
            );
            return Ok(());
        };
        match self.engine.clear_pull_request_override(&session_id) {
            Ok(message) => self.set_info(message),
            Err(err) => self.set_error(format!("{err:#}")),
        }
        Ok(())
    }

    /// `resume-pull-request-autodetection`: the way back from a detach. Turns
    /// detection on again for the selected agent and runs one check right
    /// away. Synchronous and reversible, so no modal and no confirmation.
    pub(crate) fn resume_pull_request_autodetection(&mut self) -> Result<()> {
        let Some(session_id) = self.selected_session().map(|s| s.id.clone()) else {
            self.set_error(
                "No agent session selected. Select an agent to resume its pull-request \
                 autodetection.",
            );
            return Ok(());
        };
        match self.engine.resume_pr_autodetection(&session_id) {
            Ok(message) => self.set_info(message),
            Err(err) => self.set_error(format!("{err:#}")),
        }
        Ok(())
    }

    pub(crate) fn open_kill_running(&mut self) -> Result<()> {
        let runtimes = self.running_runtime_snapshot();
        if runtimes.is_empty() {
            self.set_error(
                "No running agents or companion terminals are available to kill. Start one first, then reopen the command palette.",
            );
            return Ok(());
        }

        self.prompt = PromptState::KillRunning(KillRunningPrompt {
            runtimes,
            list: SearchableList::new(),
            selected_ids: HashSet::new(),
            focus: KillRunningFocus::List,
        });
        let select = self.bindings.label_for(Action::ToggleMarked);
        let search = self.bindings.label_for(Action::SearchToggle);
        let next = self.bindings.label_for(Action::FocusNext);
        let prev = self.bindings.label_for(Action::FocusPrev);
        self.set_info(format!(
            "Kill Running opened. Press {select} to toggle runtimes, {search} to search, and {next}/{prev} to move between the list and actions.",
        ));
        Ok(())
    }

    pub(crate) fn running_runtime_snapshot(&self) -> Vec<KillableRuntime> {
        let mut runtimes = Vec::new();

        for session in &self.engine.sessions {
            let main_running = self.engine.providers.contains_key(&session.id);
            let has_live_support = self
                .engine
                .tab_ids_for_session(&session.id)
                .into_iter()
                .any(|tab_id| tab_id != session.id && self.engine.providers.contains_key(&tab_id));
            // Skip only when NEITHER the session-slot tab nor any extra tab is live.
            if !main_running && !has_live_support {
                continue;
            }
            let project_name = self.engine.project_name_for_session(session);
            let agent_name = self.session_label(session);
            if main_running {
                let provider_name = session.provider.as_str();
                let label = Self::title_case_word(provider_name);
                let context = format!("on agent \"{agent_name}\" under project \"{project_name}\"");
                let search_text = format!(
                    "{} {} {} {} {}",
                    label,
                    context,
                    provider_name,
                    agent_name,
                    KillableRuntimeKind::Agent.noun()
                );
                runtimes.push(KillableRuntime {
                    id: RuntimeTargetId::Agent(session.id.clone()),
                    kind: KillableRuntimeKind::Agent,
                    label,
                    context,
                    search_text,
                });
            }

            // extra tabs are independent live provider processes keyed by tab
            // id. List each running one so a runaway extra tab can be killed;
            // killing it stops the process but keeps the (now dormant) tab.
            for tab_id in self.engine.tab_ids_for_session(&session.id) {
                if tab_id == session.id || !self.engine.providers.contains_key(&tab_id) {
                    continue;
                }
                let Some(tab) = self.engine.agent_tabs.get(&tab_id) else {
                    continue;
                };
                let tab_provider = tab.provider.as_str();
                let tab_label = format!("{} tab", Self::title_case_word(tab_provider));
                let tab_context =
                    format!("on agent \"{agent_name}\" under project \"{project_name}\"");
                let tab_search = format!(
                    "{} {} {} {} {} tab",
                    tab_label,
                    tab_context,
                    tab_provider,
                    agent_name,
                    KillableRuntimeKind::Agent.noun()
                );
                runtimes.push(KillableRuntime {
                    id: RuntimeTargetId::Tab(tab_id.clone()),
                    kind: KillableRuntimeKind::Agent,
                    label: tab_label,
                    context: tab_context,
                    search_text: tab_search,
                });
            }
        }

        // The UNFILTERED list: the kill overlay is its own surface with its own
        // filter row, so a query typed into the sidebar must not decide which
        // processes it offers to stop.
        for (terminal_id, terminal) in self.sorted_terminal_items() {
            let context_owner = match &terminal.owner {
                TerminalOwner::Session(session_id) => {
                    let (project_name, session_label) = self
                        .engine
                        .sessions
                        .iter()
                        .find(|session| session.id == *session_id)
                        .map(|session| {
                            (
                                self.engine.project_name_for_session(session),
                                self.session_label(session),
                            )
                        })
                        .unwrap_or_else(|| ("unknown".to_string(), session_id.clone()));
                    format!("on agent \"{session_label}\" under project \"{project_name}\"")
                }
                TerminalOwner::Project(project_id) => {
                    let project_name = self
                        .engine
                        .projects
                        .iter()
                        .find(|project| project.id == *project_id)
                        .map(|project| project.name.clone())
                        .unwrap_or_else(|| project_id.clone());
                    format!("at the repo root of project \"{project_name}\"")
                }
                // No owner to name, so the kill overlay says where it is instead,
                // which is the same thing its sidebar row says.
                TerminalOwner::Standalone => format!(
                    "standalone, in {}",
                    dux_core::home_path::shorten_home(terminal.client.spawn_dir())
                ),
            };
            // Normalize the foreground through the shared core rule (trim + strip
            // "TERM "/"term "; None when blank), so the kill overlay, the sidebar,
            // and the web all agree on the app name. None (idle shell) reads
            // "shell" in this list.
            let label = dux_core::terminal_title::terminal_foreground_display(
                terminal.foreground_cmd.as_deref(),
            )
            .unwrap_or_else(|| "shell".to_string());
            let context = context_owner;
            let search_text = format!(
                "{} {} {} {}",
                label,
                context,
                terminal.label,
                KillableRuntimeKind::Terminal.noun()
            );
            runtimes.push(KillableRuntime {
                id: RuntimeTargetId::Terminal(terminal_id.clone()),
                kind: KillableRuntimeKind::Terminal,
                label,
                context,
                search_text,
            });
        }

        runtimes.sort_by(|a, b| {
            (
                a.context.to_lowercase(),
                a.kind.noun(),
                a.label.to_lowercase(),
            )
                .cmp(&(
                    b.context.to_lowercase(),
                    b.kind.noun(),
                    b.label.to_lowercase(),
                ))
        });
        runtimes
    }

    pub(crate) fn visible_kill_running_indices(prompt: &KillRunningPrompt) -> Vec<usize> {
        prompt
            .list
            .visible_indices(&prompt.runtimes, kill_running_matches)
    }

    fn title_case_word(word: &str) -> String {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
            None => String::new(),
        }
    }

    pub(crate) fn clamp_kill_running_prompt(prompt: &mut KillRunningPrompt) {
        let visible_len = Self::visible_kill_running_indices(prompt).len();
        prompt.list.clamp_selected(visible_len);
    }

    pub(crate) fn open_confirm_kill_running_action(
        &mut self,
        action: KillRunningAction,
    ) -> Result<()> {
        let PromptState::KillRunning(prompt) = &self.prompt else {
            return Ok(());
        };
        let prompt = prompt.clone();
        let visible_indices = Self::visible_kill_running_indices(&prompt);
        let target_ids = match action {
            KillRunningAction::Hovered => visible_indices
                .get(prompt.list.selected)
                .map(|&index| vec![prompt.runtimes[index].id.clone()])
                .unwrap_or_default(),
            KillRunningAction::Selected => prompt
                .runtimes
                .iter()
                .filter(|runtime| prompt.selected_ids.contains(&runtime.id))
                .map(|runtime| runtime.id.clone())
                .collect(),
            KillRunningAction::Visible => visible_indices
                .iter()
                .map(|&index| prompt.runtimes[index].id.clone())
                .collect(),
        };

        if target_ids.is_empty() {
            let message = match action {
                KillRunningAction::Hovered => {
                    "No running agent or terminal is highlighted. Move to a visible row first."
                }
                KillRunningAction::Selected => {
                    "No running agents or terminals are selected. Press Space to select one or more runtimes first."
                }
                KillRunningAction::Visible => {
                    "No running agents or terminals are visible for the current filter. Clear or change the search first."
                }
            };
            self.set_error(message);
            return Ok(());
        }

        self.prompt = PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
            previous: prompt,
            action,
            target_ids,
            focus: ConfirmFocus::Cancel,
        });
        self.set_info(format!(
            "{} is ready. Review the warning and press Enter to confirm, or Esc to keep your running sessions alive.",
            action.button_label()
        ));
        Ok(())
    }

    pub(crate) fn kill_runtime_targets(
        &mut self,
        target_ids: &[RuntimeTargetId],
    ) -> (usize, usize) {
        let selected_session_id = self.selected_session().map(|session| session.id.clone());
        let active_terminal_id = self.active_terminal_id.clone();
        let mut killed_agents = 0;
        let mut killed_terminals = 0;
        let mut selected_agent_killed = false;
        let mut active_terminal_killed = false;

        for target_id in target_ids {
            match target_id {
                // Both the session-slot tab (Agent) and an extra tab (Tab) tear
                // down through the single-source `Engine::kill_tab_runtime`: it
                // SIGKILLs the provider, clears every runtime map (including the
                // in-flight `AgentLaunch` key a hand-rolled list used to miss),
                // detaches the agent only when this was its last live tab, and
                // clears `desired_running` on detach so the startup auto-reopen
                // pass does not relaunch the agent the user just killed. Killing
                // an extra tab KEEPS its `agent_tabs` row (the tab goes dormant;
                // row deletion is `close_tab`'s job).
                RuntimeTargetId::Agent(session_id) => {
                    if self.engine.kill_tab_runtime(session_id).killed {
                        killed_agents += 1;
                        if selected_session_id.as_deref() == Some(session_id.as_str()) {
                            selected_agent_killed = true;
                        }
                    }
                }
                RuntimeTargetId::Tab(tab_id) => {
                    if self.engine.kill_tab_runtime(tab_id).killed {
                        killed_agents += 1;
                    }
                }
                RuntimeTargetId::Terminal(terminal_id) => {
                    // Graceful teardown (SIGTERM + background reap via
                    // `begin_close_companion_terminal`), matching the shared
                    // `Command::DeleteTerminal` path and the Terminals tenet: a
                    // bare `companion_terminals.remove` here hard-SIGKILLed the
                    // child and skipped `clear_terminal_runtime`.
                    if self
                        .engine
                        .begin_close_companion_terminal(terminal_id)
                        .is_some()
                    {
                        killed_terminals += 1;
                        if active_terminal_id.as_deref() == Some(terminal_id.as_str()) {
                            active_terminal_killed = true;
                        }
                    }
                }
            }
        }

        if active_terminal_killed {
            self.active_terminal_id = None;
            if self.session_surface == SessionSurface::Terminal {
                self.input_target = InputTarget::None;
                self.fullscreen_overlay = FullscreenOverlay::None;
                self.session_surface = SessionSurface::Agent;
            }
        }

        if selected_agent_killed && self.session_surface == SessionSurface::Agent {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.focus = FocusPane::Left;
        }

        self.clamp_terminal_cursor();
        // The kill just removed PTYs; keep the poll-cadence flag honest right
        // away rather than waiting for the next tick.
        self.engine.sync_has_active_processes();

        (killed_agents, killed_terminals)
    }

    pub(crate) fn session_label(&self, session: &AgentSession) -> String {
        session.display_label()
    }

    /// Palette action: tear down the TUI and serve the web UI in the same
    /// process. LOCAL MODE only — loopback plus (when enabled) the machine's
    /// Tailscale address; the flip never reads the configurable [server] host.
    ///
    /// The pre-flight (Tailscale detection via `tailscale ip`, then an actual
    /// `TcpListener::bind` of each address) runs on a WORKER thread because the
    /// CLI call would otherwise block the UI loop. The worker reports back via
    /// `WorkerEvent::ServerFlipPreflightReady`; the main loop stashes the flip on
    /// success or surfaces the (actionable) error on failure, so a port collision
    /// or a missing Tailscale daemon keeps the TUI exactly where it was.
    ///
    /// In-flight guarded: a second invocation while a pre-flight worker is still
    /// pending — or while a successful flip is already stashed waiting for the run
    /// loop to act on it — is refused instead of spawning a second worker. Without
    /// the guard, two quick triggers would race to `bind` the same LOCAL MODE
    /// ports and the loser would surface a confusing EADDRINUSE.
    pub(crate) fn start_web_server(&mut self) {
        if self.server_flip_preflight_pending || self.pending_server_flip.is_some() {
            self.set_warning("Web server start already in progress.".to_string());
            return;
        }
        // Mint the flip's keyed busy op. The plain-success arm re-emits this op's
        // busy text (with the serve URLs) via `progress` and lets the spinner ride
        // until the flip; the warning/error arms resolve it. The resolver covers
        // only the two terminal-with-message outcomes (see `TuiServerFlipOutcome`).
        let op = dux_core::engine::status_op(
            "Starting the web server — your agents keep running.".to_string(),
        )
        .resolve_in_handler(|o: &TuiServerFlipOutcome| match o {
            TuiServerFlipOutcome::Warned(text) => dux_core::engine::Final::warning(text.clone()),
            TuiServerFlipOutcome::Failed(text) => dux_core::engine::Final::error(text.clone()),
        });
        self.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        self.pending_server_flip_op = Some(op);
        self.server_flip_preflight_pending = true;
        let port = self.engine.config.server.port;
        let tailscale_enabled = self.engine.config.server.tailscale_enabled;
        let tx = self.engine.worker_tx.clone();
        std::thread::spawn(move || {
            // Detect the Tailscale address off the UI thread (the CLI call is the
            // reason this runs on a worker). When detection fails but the user
            // opted in, carry a non-fatal warning naming the config key.
            let (tailscale_ip, detect_warning) = if tailscale_enabled {
                match dux_core::tailscale::detect_ip() {
                    Ok(ip) => (Some(ip), None),
                    Err(reason) => (
                        None,
                        Some(format!(
                            "Tailscale not detected ({}) — serving on loopback only. \
                             Set tailscale_enabled = false in [server] to silence this warning.",
                            reason.reason()
                        )),
                    ),
                }
            } else {
                (None, None)
            };

            // The pre-flight returns its own best-effort (Tailscale BIND-failure)
            // warnings; combine them with the detection warning into the single
            // `warning` the event carries, so a busy Tailscale port and a missing
            // Tailscale daemon both surface the same way (serving loopback-only).
            let result = match preflight_server_listeners(port, tailscale_ip) {
                Ok((listeners, urls, bind_warnings)) => {
                    let warning = combine_flip_warnings(detect_warning, bind_warnings);
                    let _ = tx.send(WorkerEvent::ServerFlipPreflightReady {
                        result: Ok((listeners, urls)),
                        warning,
                    });
                    return;
                }
                Err(err) => Err(format!("{err:#}")),
            };
            // A required (loopback) bind failed: surface the error; the detection
            // warning (if any) is moot because the flip is not happening.
            let _ = tx.send(WorkerEvent::ServerFlipPreflightReady {
                result,
                warning: detect_warning,
            });
        });
    }
}

/// Merge the optional Tailscale-detection warning with any best-effort
/// bind-failure warnings the pre-flight produced into the single `warning` the
/// flip event carries. Both describe the same degraded-to-loopback outcome, so
/// they are joined with a space; returns `None` when there is nothing to say.
fn combine_flip_warnings(detect: Option<String>, binds: Vec<String>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(detect);
    parts.extend(binds);
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DuxPaths;
    use crate::keybindings::{BINDING_DEFS, RuntimeBindings};
    use crate::model::{AgentSession, Project, ProviderKind, SessionStatus};
    use crate::storage::SessionStore;
    use crate::theme::Theme;
    use chrono::Utc;
    use dux_core::engine::{ProjectPersistenceOutcome, ProjectPersistenceView};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex, mpsc};
    use tempfile::tempdir;

    fn test_bindings() -> RuntimeBindings {
        RuntimeBindings::new(
            |action| {
                BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default()
            },
            true,
        )
    }

    fn test_app_with_sessions(sessions: Vec<AgentSession>, projects: Vec<Project>) -> App {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
        let bindings = test_bindings();
        let (worker_tx, worker_rx) = mpsc::channel();
        let single_instance_lock = crate::lockfile::SingleInstanceLock::acquire(&paths.lock_path)
            .expect("single-instance lock for test App");
        let config_writer =
            dux_core::config_queue::ConfigWriteQueue::new(paths.config_path.clone());
        let engine = dux_core::engine::Engine {
            config: Config::default(),
            paths,
            session_store,
            projects,
            sessions,
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            terminal_counter: 0,
            github_integration_enabled: false,
            single_instance_lock,
            surface_kind: dux_core::term_identity::SurfaceKind::Tui,
            resource_collector: Default::default(),
            host_env: dux_core::term_identity::HostEnvProbe::default(),
            worker_tx,
            worker_rx,
            config_writer,
            surface: Box::new(crate::TuiConfigSurface),
            reloading: false,
            deferred_commands: Vec::new(),
            reload_guard: None,
            providers: std::collections::HashMap::new(),
            running_provider_pins: std::collections::HashMap::new(),
            launched_drop_paste: Default::default(),
            companion_terminals: std::collections::HashMap::new(),
            agent_tabs: std::collections::HashMap::new(),
            terminating_ptys: Vec::new(),
            pending_group_removals: Vec::new(),
            gh_status: crate::model::GhStatus::Unknown,
            gh_probe: Default::default(),
            pr_statuses: std::collections::HashMap::new(),
            pr_overrides: std::collections::HashMap::new(),
            pr_suppressions: std::collections::HashSet::new(),
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync: Arc::new(Default::default()),
            pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            refs_watcher: None,
            refs_watch_paths: std::collections::HashMap::new(),
            resume_fallback_candidates: std::collections::HashMap::new(),
            pending_deletions: std::collections::HashSet::new(),
            folder_repo_statuses: std::collections::HashMap::new(),
            closing_sessions: std::collections::HashSet::new(),
            deletion_busy_messages: std::collections::HashMap::new(),
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            watched_session_id: None,
            current_origin: Default::default(),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            in_flight: std::collections::HashSet::new(),
            rename_expected: std::collections::HashMap::new(),
            pr_last_checked: std::collections::HashMap::new(),
            changed_files_poller_started: AtomicBool::new(false),
            branch_sync_worker_started: AtomicBool::new(false),
            pty_activity: std::collections::HashMap::new(),
            pty_input: std::collections::HashMap::new(),
            pty_pointer: std::collections::HashMap::new(),
            needs_attention: std::collections::HashSet::new(),
            pty_progress: std::collections::HashMap::new(),
            agent_viewed: std::collections::HashMap::new(),
            last_foreground_refresh: None,
            pending_web_checkout_ops: std::collections::HashMap::new(),
            pending_web_add_project_ops: std::collections::HashMap::new(),
            pending_web_pr_lookup_ops: std::collections::HashMap::new(),
            pending_pr_attach_ops: std::collections::HashMap::new(),
            pending_delete_ops_web: std::collections::HashMap::new(),
            pending_create_ops: std::collections::HashMap::new(),
            pending_web_launch_ops: std::collections::HashMap::new(),
            last_created_op_id: None,
            created_session_by_op: std::collections::HashMap::new(),
        };
        let mut app = App {
            engine,
            bindings,
            selected_left: 0,
            left_section: crate::app::LeftSection::Projects,
            selected_terminal_index: 0,
            right_section: RightSection::Unstaged,
            files_index: 0,
            files_search: TextInput::new(),
            files_search_active: false,
            commit_input: TextInput::new()
                .with_multiline(4)
                .with_placeholder("Type your commit message\u{2026}"),
            show_diff_line_numbers: false,
            left_width_pct: 20,
            right_width_pct: 23,
            terminal_pane_height_pct: 35,
            staged_pane_height_pct: 50,
            commit_pane_height_pct: 40,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            left_collapsed: false,
            right_collapsed: false,
            right_hidden: false,
            resize_mode: false,
            help_scroll: None,
            last_help_height: 0,
            last_help_lines: 0,
            last_first_load_height: 0,
            last_first_load_lines: 0,
            last_error_dialog_height: 0,
            last_error_dialog_lines: 0,
            pending_first_load: None,
            notes_fetch_rx: None,
            deferred_first_load_notes: None,
            notes_fetch_explicit_request: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            fullscreen_overlay: FullscreenOverlay::None,
            startup_log_viewer: None,
            status: crate::statusline::KeyedStatusController::with_clear_after(
                std::time::Duration::ZERO,
            ),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            session_surface: crate::model::SessionSurface::Agent,
            clipboard: Clipboard::new(),
            active_terminal_id: None,
            focused_tabs: std::collections::HashMap::new(),
            host_forward_carry: Vec::new(),
            host_forward_error_logged_at: None,
            agent_tab_regions: Vec::new(),
            terminal_return_to_list: false,
            last_pty_size: (0, 0),
            last_pty_resize_target: None,
            grid_generation: 0,
            scroll_mode: std::collections::HashSet::new(),
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            start_time: std::time::Instant::now(),
            refusal_blink: None,
            inactive_collapsed: false,
            inactive_search_dismissed: None,
            inactive_collapse_overridden: false,
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            overlay_layout: OverlayMouseLayoutState::default(),
            mouse_drag: None,
            center_mouse_forward: None,
            last_mouse_click: None,
            pressed_button: None,
            interactive_patterns: crate::keybindings::InteractiveBytePatterns {
                bindings: Vec::new(),
            },
            raw_input_parser: crate::raw_input::RawInputParser::default(),
            raw_input_buf: Vec::new(),
            loading_input_buf: Vec::new(),
            in_bracket_paste: false,
            raw_paste_normalize: false,
            raw_paste_prev_cr: false,
            terminal_focus: crate::focus::TerminalFocus::new(),
            macro_bar: None,
            sigwinch_flag: Arc::new(AtomicBool::new(false)),
            sigwinch_sig_id: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            shutdown_sig_ids: Vec::new(),
            force_redraw: false,
            welcome_tip_index: 0,
            welcome_logo_visible: false,
            welcome_logo_alt: false,
            welcome_tip_selection: usize::MAX,
            pr_banner_at_bottom: true,
            syntax_cache: crate::diff::SyntaxCache::new(),
            snapshot_buf: crate::pty::TerminalSnapshot::empty(),
            last_snapshot_id: None,
            terminal_selection: None,
            startup_log_selection: None,
            pending_server_flip: None,
            server_flip_preflight_pending: false,
            pending_persist_ops: std::collections::HashMap::new(),
            pending_worktree_ops: std::collections::HashMap::new(),
            pending_pr_lookup_ops: std::collections::HashMap::new(),
            pending_pr_reference: None,
            pending_pr_reference_op: None,
            dispatched_pr_lookups: Vec::new(),
            pending_delete_ops: std::collections::HashMap::new(),
            pending_reconnect_ops: std::collections::HashMap::new(),
            pending_checkout_inspect_ops: std::collections::HashMap::new(),
            pending_changed_files_refresh: None,
            pending_server_flip_op: None,
            pending_config_reload_op: None,
            project_chooser_context: None,
            agent_filter: None,
        };
        app.interactive_patterns = app.bindings.interactive_byte_patterns();
        app.rebuild_left_items();
        app
    }

    fn make_session(id: &str, provider: &str, worktree: &str) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            provider: ProviderKind::from_str(provider),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "project-1".to_string(),
                    project_path: Some("/tmp/project".to_string()),
                    source_branch: "main".to_string(),
                    branch_name: format!("branch-{id}"),
                    initial_branch: format!("branch-{id}"),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: worktree.to_string(),
                },
            ),
        }
    }

    fn test_engine_with_sessions(
        sessions: Vec<AgentSession>,
        projects: Vec<Project>,
    ) -> dux_core::engine::Engine {
        let tmp = tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
        let single_instance_lock = crate::lockfile::SingleInstanceLock::acquire(&paths.lock_path)
            .expect("single-instance lock for test engine");
        let (worker_tx, worker_rx) = mpsc::channel();
        // auto_reopen on so bootstrap WOULD relaunch — proving resume's skip.
        let mut config = Config::default();
        config.ui.auto_reopen_agents = true;
        let config_writer =
            dux_core::config_queue::ConfigWriteQueue::new(paths.config_path.clone());
        dux_core::engine::Engine {
            config,
            paths,
            session_store,
            projects,
            sessions,
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            terminal_counter: 0,
            github_integration_enabled: false,
            single_instance_lock,
            surface_kind: dux_core::term_identity::SurfaceKind::Tui,
            resource_collector: Default::default(),
            host_env: dux_core::term_identity::HostEnvProbe::default(),
            worker_tx,
            worker_rx,
            config_writer,
            surface: Box::new(crate::TuiConfigSurface),
            reloading: false,
            deferred_commands: Vec::new(),
            reload_guard: None,
            providers: std::collections::HashMap::new(),
            running_provider_pins: std::collections::HashMap::new(),
            launched_drop_paste: Default::default(),
            companion_terminals: std::collections::HashMap::new(),
            agent_tabs: std::collections::HashMap::new(),
            terminating_ptys: Vec::new(),
            pending_group_removals: Vec::new(),
            gh_status: crate::model::GhStatus::Unknown,
            gh_probe: Default::default(),
            pr_statuses: std::collections::HashMap::new(),
            pr_overrides: std::collections::HashMap::new(),
            pr_suppressions: std::collections::HashSet::new(),
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync: Arc::new(Default::default()),
            pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            refs_watcher: None,
            refs_watch_paths: std::collections::HashMap::new(),
            resume_fallback_candidates: std::collections::HashMap::new(),
            pending_deletions: std::collections::HashSet::new(),
            folder_repo_statuses: std::collections::HashMap::new(),
            closing_sessions: std::collections::HashSet::new(),
            deletion_busy_messages: std::collections::HashMap::new(),
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            watched_session_id: None,
            current_origin: Default::default(),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            in_flight: std::collections::HashSet::new(),
            rename_expected: std::collections::HashMap::new(),
            pr_last_checked: std::collections::HashMap::new(),
            changed_files_poller_started: AtomicBool::new(false),
            branch_sync_worker_started: AtomicBool::new(false),
            pty_activity: std::collections::HashMap::new(),
            pty_input: std::collections::HashMap::new(),
            pty_pointer: std::collections::HashMap::new(),
            needs_attention: std::collections::HashSet::new(),
            pty_progress: std::collections::HashMap::new(),
            agent_viewed: std::collections::HashMap::new(),
            last_foreground_refresh: None,
            pending_web_checkout_ops: std::collections::HashMap::new(),
            pending_web_add_project_ops: std::collections::HashMap::new(),
            pending_web_pr_lookup_ops: std::collections::HashMap::new(),
            pending_pr_attach_ops: std::collections::HashMap::new(),
            pending_delete_ops_web: std::collections::HashMap::new(),
            pending_create_ops: std::collections::HashMap::new(),
            pending_web_launch_ops: std::collections::HashMap::new(),
            last_created_op_id: None,
            created_session_by_op: std::collections::HashMap::new(),
        }
    }

    fn seed_tab(app: &mut App, id: &str, session_id: &str, provider: &str, order: i64) {
        app.engine.agent_tabs.insert(
            id.to_string(),
            crate::model::AgentTab {
                id: id.to_string(),
                session_id: session_id.to_string(),
                provider: ProviderKind::from_str(provider),
                sort_order: order,
                created_at: Utc::now(),
            },
        );
    }

    #[test]
    fn session_tab_ids_are_main_first_then_sorted() {
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        seed_tab(&mut app, "t2", "s1", "codex", 2);
        seed_tab(&mut app, "t1", "s1", "claude", 1);
        seed_tab(&mut app, "other", "s2", "claude", 1);
        assert_eq!(
            app.session_tab_ids("s1"),
            vec!["s1".to_string(), "t1".to_string(), "t2".to_string()]
        );
    }

    #[test]
    fn focused_tab_defaults_to_main_and_clamps_when_gone() {
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        seed_tab(&mut app, "t1", "s1", "claude", 1);
        // Default is Main (the session id).
        assert_eq!(app.focused_tab_id("s1"), "s1");
        app.set_focused_tab("s1", "t1");
        assert_eq!(app.focused_tab_id("s1"), "t1");
        // A stored-but-missing tab clamps back to Main.
        app.focused_tabs
            .insert("s1".to_string(), "gone".to_string());
        assert_eq!(app.focused_tab_id("s1"), "s1");
        // Teardown prune drops the LOCAL entry. `set_focused_tab` also wrote the
        // choice through to the engine's persisted `last_focused_tab`, so the
        // resolver now falls back to that remembered value rather than Main
        // (see `focused_tab_id_falls_back_to_the_engine_remembered_tab_when_the_map_has_no_entry`).
        // In production this is harmless: the real teardown caller
        // (session delete) removes the whole `agent_sessions` row, taking
        // `last_focused_tab` with it.
        app.set_focused_tab("s1", "t1");
        app.clear_focused_tab_for_session("s1");
        assert_eq!(app.focused_tab_id("s1"), "t1");
        // With no engine memory either, it clamps to Main.
        app.engine.sessions[0].last_focused_tab = None;
        assert_eq!(app.focused_tab_id("s1"), "s1");
    }

    #[test]
    fn focused_tab_id_falls_back_to_the_engine_remembered_tab_when_the_map_has_no_entry() {
        // Simulates a post-restart App: the in-process `focused_tabs` HashMap is
        // empty, but the engine session (loaded from SQLite) carries a
        // remembered `last_focused_tab`.
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        seed_tab(&mut app, "t1", "s1", "claude", 1);
        app.engine.sessions[0].last_focused_tab = Some("t1".to_string());
        assert!(!app.focused_tabs.contains_key("s1"));

        assert_eq!(app.focused_tab_id("s1"), "t1");
    }

    #[test]
    fn focused_tab_id_falls_back_to_main_when_the_remembered_engine_tab_is_gone() {
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        app.engine.sessions[0].last_focused_tab = Some("gone".to_string());
        assert_eq!(app.focused_tab_id("s1"), "s1");
    }

    #[test]
    fn focused_tab_id_prefers_the_live_hashmap_entry_over_the_engine_remembered_tab() {
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        seed_tab(&mut app, "t1", "s1", "claude", 1);
        seed_tab(&mut app, "t2", "s1", "codex", 2);
        app.engine.sessions[0].last_focused_tab = Some("t1".to_string());
        app.set_focused_tab("s1", "t2");

        assert_eq!(app.focused_tab_id("s1"), "t2");
    }

    #[test]
    fn set_focused_tab_writes_through_to_the_engine_and_persists() {
        let mut app =
            test_app_with_sessions(vec![make_session("s1", "codex", "/tmp/w1")], Vec::new());
        app.engine
            .session_store
            .upsert_session(&app.engine.sessions[0].clone())
            .expect("seed session row");
        seed_tab(&mut app, "t1", "s1", "claude", 1);

        app.set_focused_tab("s1", "t1");
        assert_eq!(
            app.engine.sessions[0].last_focused_tab.as_deref(),
            Some("t1")
        );
        let reloaded = app.engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.last_focused_tab.as_deref(), Some("t1"));

        // Switching back to Main clears the remembered engine value too.
        app.set_focused_tab("s1", "s1");
        assert_eq!(app.engine.sessions[0].last_focused_tab, None);
        let reloaded = app.engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.last_focused_tab, None);
    }

    #[test]
    fn start_web_server_sets_busy_and_dispatches_worker() {
        // start_web_server now runs the pre-flight on a WORKER thread (it shells
        // out to `tailscale ip`), so it does not stash the flip synchronously. It
        // must immediately set a Busy status and arm nothing yet.
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.start_web_server();
        assert!(app.pending_server_flip.is_none());
        assert!(app.status.message().contains("Starting the web server"));
    }

    #[test]
    fn start_web_server_double_trigger_is_guarded() {
        // First trigger arms the in-flight guard and shows Busy. A second trigger
        // while the worker is still pending must be REFUSED (no second worker) with
        // the "already in progress" status — otherwise two workers race to bind the
        // same LOCAL MODE ports and the loser surfaces a confusing EADDRINUSE.
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.start_web_server();
        assert!(
            app.server_flip_preflight_pending,
            "first trigger arms guard"
        );
        assert!(app.status.message().contains("Starting the web server"));

        app.start_web_server();
        assert!(
            app.status
                .message()
                .contains("Web server start already in progress"),
            "second trigger while pending must be refused"
        );
        assert!(
            app.server_flip_preflight_pending,
            "guard stays armed after a refused retry"
        );

        // The worker event clears the guard (Err arm here) so a later retry works.
        app.apply_reaction(EventReaction::ServerFlipPreflightReady {
            result: Err("could not start the web server: address in use".to_string()),
            warning: None,
        });
        assert!(
            !app.server_flip_preflight_pending,
            "guard clears when the worker event lands"
        );

        // A stashed flip (success awaiting the run loop) also blocks a re-trigger.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}", listener.local_addr().unwrap());
        app.apply_reaction(EventReaction::ServerFlipPreflightReady {
            result: Ok((vec![listener], vec![url])),
            warning: None,
        });
        assert!(app.pending_server_flip.is_some());
        app.start_web_server();
        assert!(
            app.status
                .message()
                .contains("Web server start already in progress"),
            "a stashed flip must also refuse a re-trigger"
        );
    }

    /// Mint and stash a server-flip op exactly as `start_web_server` does (without
    /// spawning the real pre-flight worker), returning nothing — the op lives in
    /// `app.pending_server_flip_op`. The keyed busy is shown so the
    /// `ServerFlipPreflightReady` handler under test has a stashed op to advance.
    fn stash_server_flip_op(app: &mut App) {
        let op = dux_core::engine::status_op(
            "Starting the web server — your agents keep running.".to_string(),
        )
        .resolve_in_handler(|o: &TuiServerFlipOutcome| match o {
            TuiServerFlipOutcome::Warned(text) => dux_core::engine::Final::warning(text.clone()),
            TuiServerFlipOutcome::Failed(text) => dux_core::engine::Final::error(text.clone()),
        });
        app.apply_reaction(EventReaction::Status(op.pending_status()));
        app.pending_server_flip_op = Some(op);
        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "the keyed busy must show after dispatch"
        );
    }

    #[test]
    fn server_flip_preflight_ready_ok_progresses_busy_and_stashes_flip() {
        // The worker's plain-success path: a constructed event carrying bound
        // listeners and URLs stashes the flip and ADVANCES the keyed busy (via
        // `progress`) to the URL-bearing line — still a Busy spinner, same op,
        // which rides until the run loop flips. The op stays stashed (no success
        // final), byte-identical to today.
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        stash_server_flip_op(&mut app);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}", listener.local_addr().unwrap());
        app.apply_reaction(EventReaction::ServerFlipPreflightReady {
            result: Ok((vec![listener], vec![url.clone()])),
            warning: None,
        });

        let (listeners, urls) = app
            .pending_server_flip
            .as_ref()
            .expect("a successful pre-flight stashes the flip");
        assert_eq!(listeners.len(), 1);
        assert_eq!(urls, &vec![url.clone()]);
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Busy);
        assert_eq!(
            app.status.message(),
            format!("Starting the web server on {url} — your agents keep running.")
        );
        assert!(
            app.pending_server_flip_op.is_some(),
            "the plain-success busy rides until the flip, so the op stays stashed"
        );
    }

    #[test]
    fn server_flip_preflight_ready_warning_shows_warning_status() {
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        stash_server_flip_op(&mut app);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let url = format!("http://{}", listener.local_addr().unwrap());
        app.apply_reaction(EventReaction::ServerFlipPreflightReady {
            result: Ok((vec![listener], vec![url.clone()])),
            warning: Some("Tailscale not detected — serving on loopback only.".to_string()),
        });
        assert!(app.pending_server_flip.is_some());
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
        assert_eq!(
            app.status.message(),
            format!(
                "Tailscale not detected — serving on loopback only. Starting the web server on {url} — your agents keep running."
            )
        );
        assert!(
            app.pending_server_flip_op.is_none(),
            "the warning final consumes the op"
        );
    }

    #[test]
    fn server_flip_preflight_ready_err_surfaces_error_and_stays_up() {
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        stash_server_flip_op(&mut app);
        app.apply_reaction(EventReaction::ServerFlipPreflightReady {
            result: Err("could not start the web server: address in use".to_string()),
            warning: None,
        });
        assert!(
            app.pending_server_flip.is_none(),
            "a failed pre-flight must not arm the flip"
        );
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert_eq!(
            app.status.message(),
            "could not start the web server: address in use"
        );
        assert!(
            app.pending_server_flip_op.is_none(),
            "the error final consumes the op"
        );
    }

    #[test]
    fn finish_add_project_ends_on_final_status_not_stuck_busy() {
        // Regression: the add is INLINE, so the reaction already carries the
        // FINAL status (the `Added` arm's success info). A trailing
        // `set_busy("Saving project…")` after `apply_reaction` would run last and
        // never resolve, leaving a stuck spinner. The post-add status must be the
        // success Info, not a Busy.
        // `finish_add_project_with_status` only persists the project; it does not
        // validate the path as a git repo, so a plain tempdir suffices.
        let repo = tempdir().expect("repo tempdir");
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());

        app.finish_add_project_with_status(
            repo.path().to_string_lossy().into_owned(),
            "Demo".to_string(),
            "main".to_string(),
            "main".to_string(),
            "Added project \"Demo\" to the workspace.".to_string(),
        )
        .expect("finish add");

        assert_eq!(
            app.status.tone(),
            dux_core::statusline::StatusTone::Info,
            "post-add status must be the final Info, not a stuck Busy: {:?} {}",
            app.status.tone(),
            app.status.message()
        );
        assert!(
            app.status.message().contains("Added project \"Demo\""),
            "expected the success message to remain, got: {}",
            app.status.message()
        );
    }

    /// Create an unborn git repo (init + identity, NO commit) and return its
    /// path string.
    fn init_unborn_repo() -> (tempfile::TempDir, String) {
        fn run_git(cwd: &Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        }
        let repo = tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-b", "main"]);
        run_git(repo.path(), &["config", "user.name", "test"]);
        run_git(repo.path(), &["config", "user.email", "t@t"]);
        let path = repo.path().to_string_lossy().to_string();
        (repo, path)
    }

    #[test]
    fn add_project_on_unborn_repo_prompts_to_create_initial_commit() {
        let (_repo, path) = init_unborn_repo();
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());

        app.add_project(path.clone(), "Fresh".to_string())
            .expect("add_project");

        assert!(
            matches!(app.prompt, PromptState::ConfirmCreateInitialCommit { .. }),
            "an unborn repo must prompt to create the initial commit, got {:?}",
            app.prompt
        );
        // Nothing is registered until the user confirms.
        assert!(
            app.engine.projects.is_empty(),
            "the project must not be added before the commit is confirmed"
        );
    }

    #[test]
    fn resolving_create_initial_commit_births_head_and_adds_project() {
        let (_repo, path) = init_unborn_repo();
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.add_project(path.clone(), "Fresh".to_string())
            .expect("add_project");

        // Confirming dispatches a background worker; the commit + registration
        // complete asynchronously. Drain until the project appears (bounded).
        app.resolve_confirm_create_initial_commit(true);
        assert!(matches!(app.prompt, PromptState::None));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.engine.projects.is_empty() && std::time::Instant::now() < deadline {
            app.drain_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            dux_core::git::repo_has_commits(Path::new(&path)),
            "confirming must create the initial commit"
        );
        assert_eq!(
            app.engine.projects.len(),
            1,
            "the project must be registered after the commit completes"
        );
        // The project is registered on its REAL branch (not the leading-branch
        // value reused for both fields).
        assert_eq!(app.engine.projects[0].current_branch, "main");
        assert_eq!(
            app.engine.projects[0].leading_branch.as_deref(),
            Some("main")
        );
        // The per-path serialization gate is released once the worker completes.
        assert!(
            !app.engine
                .is_in_flight(&dux_core::engine::InFlightKey::InitialCommit(path.clone())),
            "the in-flight gate must be cleared after completion"
        );
    }

    #[test]
    fn create_initial_commit_gate_is_released_after_a_failed_commit() {
        // A failed bootstrap (read-only object store) must still clear the
        // in-flight gate so the user can retry, and must surface an error.
        if std::process::Command::new("id")
            .arg("-u")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
            .unwrap_or(false)
        {
            return; // root bypasses the read-only trick
        }
        let (_repo, path) = init_unborn_repo();
        let objects = Path::new(&path).join(".git/objects");
        let original = std::fs::metadata(&objects).unwrap().permissions();
        let mut ro = original.clone();
        ro.set_readonly(true);
        std::fs::set_permissions(&objects, ro).unwrap();

        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.add_project(path.clone(), "Fresh".to_string())
            .expect("add_project");
        app.resolve_confirm_create_initial_commit(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app
            .engine
            .is_in_flight(&dux_core::engine::InFlightKey::InitialCommit(path.clone()))
            && std::time::Instant::now() < deadline
        {
            app.drain_events();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        std::fs::set_permissions(&objects, original).unwrap();

        assert!(
            !app.engine
                .is_in_flight(&dux_core::engine::InFlightKey::InitialCommit(path.clone())),
            "a failed commit must still release the in-flight gate"
        );
        assert!(
            app.engine.projects.is_empty(),
            "a failed commit adds nothing"
        );
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Error);
    }

    #[test]
    fn unborn_repo_on_nonstandard_branch_prompts_for_commit_not_branch_warning() {
        // A fresh `git init -b trunk` is unborn, so the no-commits prompt takes
        // precedence over the non-default-branch heuristic warning — the user
        // just created this branch; warning "that's not main" would be noise.
        fn run_git(cwd: &Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        }
        let repo = tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-b", "trunk"]);
        run_git(repo.path(), &["config", "user.name", "test"]);
        run_git(repo.path(), &["config", "user.email", "t@t"]);
        let path = repo.path().to_string_lossy().to_string();

        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.add_project(path, "Trunk".to_string()).expect("add");
        assert!(
            matches!(app.prompt, PromptState::ConfirmCreateInitialCommit { .. }),
            "unborn repo must prompt for a commit, not the branch warning, got {:?}",
            app.prompt
        );
    }

    #[test]
    fn cancelling_create_initial_commit_leaves_repo_and_workspace_untouched() {
        let (_repo, path) = init_unborn_repo();
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        app.add_project(path.clone(), "Fresh".to_string())
            .expect("add_project");

        app.resolve_confirm_create_initial_commit(false);

        assert!(
            !dux_core::git::repo_has_commits(Path::new(&path)),
            "cancelling must NOT create a commit"
        );
        assert!(
            app.engine.projects.is_empty(),
            "cancelling must not register the project"
        );
        assert!(matches!(app.prompt, PromptState::None));
    }

    /// Mint and stash a checkout/inspect op exactly as the three dispatch sites
    /// do, returning its opaque id. Used by the resolution-wiring tests below so
    /// they exercise `drain_events` without spawning git workers.
    fn stash_checkout_inspect_op(app: &mut App, busy: &str) -> String {
        let op = dux_core::engine::status_op(busy.to_string()).resolve_in_handler(
            |o: &TuiCheckoutInspectOutcome| match o {
                TuiCheckoutInspectOutcome::Done => dux_core::engine::Final::clear(),
            },
        );
        let pending = op.pending_status();
        let id = op.id().to_string();
        app.pending_checkout_inspect_ops.insert(id.clone(), op);
        app.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        assert_eq!(
            app.status.tone(),
            dux_core::statusline::StatusTone::Busy,
            "the keyed busy must show after dispatch"
        );
        id
    }

    /// Site 3 short-circuit (already-leading): the inspection op resolves to a
    /// clear, and the visible final is the engine's byte-identical info line.
    #[test]
    fn checkout_inspect_op_already_leading_clears_busy_and_shows_engine_message() {
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(Vec::new(), vec![project.clone()]);
        let id = stash_checkout_inspect_op(
            &mut app,
            &format!(
                "Checking the default branch for project \"{}\"...",
                project.name
            ),
        );

        app.engine
            .worker_tx
            .send(WorkerEvent::CheckoutProjectDefaultBranchInspected {
                project: project.clone(),
                result: Ok(("main".to_string(), None)),
                status_op_id: Some(id.clone()),
            })
            .unwrap();
        app.drain_events();

        assert!(
            !app.pending_checkout_inspect_ops.contains_key(&id),
            "the op must be consumed so its busy never strands"
        );
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Info);
        assert_eq!(
            app.status.message(),
            "Project \"demo\" is already on the leading branch \"main\"."
        );
    }

    /// Site 3 short-circuit (inspect failed): clears the busy; the engine's
    /// byte-identical error line shows.
    #[test]
    fn checkout_inspect_op_inspect_failed_clears_busy_and_shows_engine_error() {
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(Vec::new(), vec![project.clone()]);
        let id = stash_checkout_inspect_op(
            &mut app,
            &format!(
                "Checking the default branch for project \"{}\"...",
                project.name
            ),
        );

        app.engine
            .worker_tx
            .send(WorkerEvent::CheckoutProjectDefaultBranchInspected {
                project: project.clone(),
                result: Err("git exploded".to_string()),
                status_op_id: Some(id.clone()),
            })
            .unwrap();
        app.drain_events();

        assert!(!app.pending_checkout_inspect_ops.contains_key(&id));
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Error);
        assert_eq!(
            app.status.message(),
            "Couldn't inspect the default branch for project \"demo\": git exploded"
        );
    }

    /// Site 3 Known case CHAINS into worker 2: the op must SURVIVE the inspection
    /// completion (the `DispatchProjectDefaultBranchCheckout` reaction keeps it
    /// alive), with its busy text re-emitted as worker 2's "Checking out…" line on
    /// the SAME id — one continuous spinner, changing text. Then worker 2's real
    /// `git switch` completion clears it. Uses a real repo so worker 2 is
    /// deterministic (no synthetic event racing the spawned worker).
    #[test]
    fn checkout_inspect_op_known_case_keeps_one_spinner_across_the_chain() {
        fn run_git(cwd: &Path, args: &[&str]) {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        }
        let repo = tempdir().expect("repo tempdir");
        run_git(repo.path(), &["init", "-b", "main"]);
        run_git(repo.path(), &["config", "user.name", "test"]);
        run_git(repo.path(), &["config", "user.email", "t@t"]);
        run_git(repo.path(), &["commit", "--allow-empty", "-m", "init"]);
        run_git(repo.path(), &["switch", "-c", "feature"]);
        let repo_path = repo.path().to_string_lossy().to_string();

        let mut project = make_project("project-1", "claude");
        project.path = repo_path.clone();
        let mut app = test_app_with_sessions(Vec::new(), vec![project.clone()]);
        let id = stash_checkout_inspect_op(
            &mut app,
            &format!(
                "Checking the default branch for project \"{}\"...",
                project.name
            ),
        );

        // Worker 1 found a Known default different from the current branch; this
        // chains into worker 2 (spawned by the reaction handler).
        app.engine
            .worker_tx
            .send(WorkerEvent::CheckoutProjectDefaultBranchInspected {
                project: project.clone(),
                result: Ok((
                    "feature".to_string(),
                    Some(dux_core::worker::BranchWarningKind::Known {
                        default_branch: "main".to_string(),
                    }),
                )),
                status_op_id: Some(id.clone()),
            })
            .unwrap();
        app.drain_events();

        // The op SURVIVES (the chain handoff owns it now) and the spinner text
        // advanced to worker 2's busy on the SAME opaque id.
        assert!(
            app.pending_checkout_inspect_ops.contains_key(&id),
            "the op must survive the inspect→switch handoff"
        );
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Busy);
        assert_eq!(
            app.status.message(),
            format!("Checking out \"main\" in {repo_path} for the selected project...")
        );

        // Drain worker 2's real completion (poll briefly; it runs off-thread).
        for _ in 0..200 {
            app.drain_events();
            if !app.pending_checkout_inspect_ops.contains_key(&id) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(
            !app.pending_checkout_inspect_ops.contains_key(&id),
            "worker 2's completion must consume the op"
        );
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Info);
        assert_eq!(
            app.status.message(),
            "Checked out \"main\" for project \"demo\"."
        );
    }

    /// Site 1 (checkout-default switch FAILURE): clears the busy; the engine's
    /// byte-identical error line shows.
    #[test]
    fn checkout_inspect_op_switch_failure_clears_busy_and_shows_engine_error() {
        let mut project = make_project("project-1", "claude");
        project.path = "/tmp/switch-fail-test".to_string();
        let mut app = test_app_with_sessions(Vec::new(), vec![project.clone()]);
        let id = stash_checkout_inspect_op(
            &mut app,
            "Checking out \"main\" in /tmp/switch-fail-test for the selected project...",
        );

        app.engine
            .worker_tx
            .send(WorkerEvent::NonDefaultBranchCheckoutCompleted {
                action: NonDefaultBranchAction::CheckoutProjectDefault { project },
                target_branch: "main".to_string(),
                result: Err("switch refused".to_string()),
                status_op_id: Some(id.clone()),
            })
            .unwrap();
        app.drain_events();

        assert!(!app.pending_checkout_inspect_ops.contains_key(&id));
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Error);
        assert_eq!(
            app.status.message(),
            "Couldn't check out \"main\" in /tmp/switch-fail-test — resolve in your terminal and retry."
        );
    }

    /// Site 2 (create-agent branch inspection FAILURE): clears the busy; the
    /// engine's byte-identical error line shows.
    #[test]
    fn create_agent_inspect_op_failure_clears_busy_and_shows_engine_error() {
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(Vec::new(), vec![project.clone()]);
        let id = stash_checkout_inspect_op(
            &mut app,
            &format!(
                "Checking the current branch for project \"{}\" before creating an agent...",
                project.name
            ),
        );

        app.engine
            .worker_tx
            .send(WorkerEvent::CreateAgentBranchInspected {
                project,
                result: Err("inspection blew up".to_string()),
                status_op_id: Some(id.clone()),
            })
            .unwrap();
        app.drain_events();

        assert!(!app.pending_checkout_inspect_ops.contains_key(&id));
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Error);
        assert_eq!(app.status.message(), "inspection blew up");
    }

    #[test]
    fn finish_add_project_surfaces_rollback_error_on_config_write_failure() {
        // The TUI failure path: when the inline config write fails, the engine
        // rolls back and returns an error `Status`; `apply_reaction` must surface
        // it as an Error on the status line (not a stuck Busy, not a false Info),
        // and nothing must persist.
        let repo = tempdir().expect("repo tempdir");
        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        // Point the writer at a nonexistent directory so the eager save fails with
        // an I/O error, forcing the rollback path. (`with_dead_writer` is
        // cfg(test)-gated to dux-core and not visible from this crate's tests.)
        app.engine.config_writer =
            dux_core::config_queue::ConfigWriteQueue::new("/nonexistent/dir/cfg.toml".into());

        app.finish_add_project_with_status(
            repo.path().to_string_lossy().into_owned(),
            "Demo".to_string(),
            "main".to_string(),
            "main".to_string(),
            "Added project \"Demo\" to the workspace.".to_string(),
        )
        .expect("finish add");

        assert_eq!(
            app.status.tone(),
            dux_core::statusline::StatusTone::Error,
            "a rolled-back add must show an Error, got {:?}: {}",
            app.status.tone(),
            app.status.message()
        );
        assert!(
            !app.status.message().contains("Added project \"Demo\""),
            "the optimistic success message leaked on a failed add: {}",
            app.status.message()
        );
        // The rollback undid the in-memory list and the SQLite row.
        assert!(app.engine.projects.is_empty());
        assert!(
            app.engine
                .session_store
                .load_projects()
                .expect("load projects")
                .is_empty()
        );
    }

    #[test]
    fn finish_add_project_writes_config_once_through_the_queue() {
        // Regression: the engine handler already writes config.toml through the
        // eager queue (authoritative, with SQLite rollback). The `Added` reaction
        // arm must NOT also write it off-queue via
        // `persist_config_projects_from_runtime` — that was a DOUBLE write.
        //
        // The two writes leave byte-identical content, so the only observable that
        // distinguishes one write from two is the WRITE COUNT. We isolate the
        // off-queue write: point the eager queue at a DIFFERENT, writable path
        // than `config_path`, so the handler's (queue) write lands elsewhere and
        // leaves `config_path` untouched. Then `config_path` exists on disk if and
        // only if the off-queue `persist_config_projects_from_runtime` ran. With
        // the fix it must NOT exist; under the bug it would.
        let repo = tempdir().expect("repo tempdir");
        let raw_path = repo.path().to_string_lossy().into_owned();

        let mut app = test_app_with_sessions(Vec::new(), Vec::new());
        // Redirect the eager queue to a separate file so only the off-queue write
        // (if any) would touch `config_path`. `config_path` is the file ONLY an
        // off-queue `save_config` would create, so its absence after the add is
        // the oracle. (No pre-check needed: the test infra never writes it.)
        let queue_target = repo.path().join("queued-config.toml");
        app.engine.config_writer =
            dux_core::config_queue::ConfigWriteQueue::new(queue_target.clone());

        app.finish_add_project_with_status(
            raw_path.clone(),
            "Demo".to_string(),
            "main".to_string(),
            "main".to_string(),
            "Added project \"Demo\" to the workspace.".to_string(),
        )
        .expect("finish add");
        app.engine.config_writer.flush();

        // The handler's authoritative (queue) write landed on the redirected path.
        assert!(
            queue_target.exists(),
            "the inline-Add handler must write config through the queue"
        );
        // The `Added` arm must NOT have written config off-queue: with the fix the
        // original config_path is never touched.
        assert!(
            !app.engine.paths.config_path.exists(),
            "the Added arm wrote config off-queue (double write) — config_path \
             should never be touched after the queue write"
        );
        // And the add still succeeded end to end. The path is stored in the
        // portable form (the queue handler now portabilizes it, matching what the
        // old off-queue write produced), so compare against that mapping rather
        // than the raw absolute path.
        assert_eq!(app.engine.config.projects.len(), 1);
        assert_eq!(
            app.engine.config.projects[0].path,
            portable_project_path(&raw_path)
        );
        assert_eq!(app.status.tone(), dux_core::statusline::StatusTone::Info);
    }

    #[test]
    fn combine_flip_warnings_none_when_empty() {
        assert_eq!(combine_flip_warnings(None, Vec::new()), None);
    }

    #[test]
    fn combine_flip_warnings_passes_detection_warning_through() {
        let detect = Some("Tailscale not detected — serving on loopback only.".to_string());
        let combined = combine_flip_warnings(detect, Vec::new()).expect("warning present");
        assert!(combined.contains("Tailscale not detected"));
    }

    #[test]
    fn combine_flip_warnings_merges_detection_and_bind_failures() {
        // A best-effort Tailscale BIND failure (the new bug) joins the detection
        // warning into a single string so both reach the status line.
        let detect = Some("detect warning.".to_string());
        let binds = vec!["bind warning A.".to_string(), "bind warning B.".to_string()];
        let combined = combine_flip_warnings(detect, binds).expect("warning present");
        assert!(combined.contains("detect warning."));
        assert!(combined.contains("bind warning A."));
        assert!(combined.contains("bind warning B."));
    }

    #[test]
    fn combine_flip_warnings_bind_only() {
        // When Tailscale WAS detected but the bind to it failed, there is no
        // detection warning — only the bind-failure warning surfaces.
        let binds = vec!["the Tailscale port is busy.".to_string()];
        let combined = combine_flip_warnings(None, binds).expect("warning present");
        assert_eq!(combined, "the Tailscale port is busy.");
    }

    #[test]
    fn resume_skips_session_restore_and_rebuilds_view() {
        // A live session arrives from the web server already Running with
        // desired_running set. bootstrap's restore_sessions would flip its
        // status (worktree missing → Exited) and possibly relaunch it; resume
        // must touch neither — the provider is already alive.
        let mut session = make_session("agent-1", "codex", "/tmp/nonexistent-worktree");
        session.status = SessionStatus::Active;
        session.desired_running = true;
        let project = make_project("project-1", "codex");
        let engine = test_engine_with_sessions(vec![session], vec![project]);

        let app = App::resume(engine).expect("resume builds an App");

        // restore_sessions was skipped: the status is untouched (NOT flipped to
        // Exited despite the missing worktree).
        assert_eq!(
            app.engine.sessions[0].status,
            SessionStatus::Active,
            "resume must not re-run restore_sessions"
        );
        // No provider was launched and no launch work was dispatched.
        assert!(
            app.engine.providers.is_empty(),
            "resume must not spawn PTYs"
        );
        assert!(
            app.engine.worker_rx.try_recv().is_err(),
            "resume must not post any worker event (no agent relaunch)"
        );
        // View state was rebuilt: the session shows up in the left pane cache.
        assert!(
            !app.left_items_cache.is_empty(),
            "resume must rebuild the left-pane items"
        );
        // The status line carries the verbose resume message.
        assert!(
            app.status.message().contains("Web server stopped"),
            "resume should arrive with the agents-kept-running message"
        );

        // The first-load gate is pinned INSIDE the `SessionRestore::Restore`
        // guard. `test_engine_with_sessions` opens a brand-new store, so this
        // engine has NO `last_seen_version` — the fresh-install shape that would
        // otherwise show the welcome screen (and, on an upgrade, dispatch the
        // release-notes fetch). A web-server→TUI flip must show neither, and must
        // not stamp the version: the user may still be looking at that screen in
        // the browser, and stamping here would consume it for both surfaces.
        // These assertions pass today; they fail the moment `begin_first_load`
        // moves out of the guard.
        assert!(
            matches!(app.prompt, PromptState::None),
            "a resume must not open a first-load screen, got {:?}",
            app.prompt
        );
        assert!(
            app.pending_first_load.is_none(),
            "a resume must not dispatch the release-notes fetch"
        );
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "a resume must not stamp the running version as seen"
        );
    }

    fn make_project(id: &str, provider: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "demo".to_string(),
            path: "/tmp/project".to_string(),
            explicit_default_provider: Some(ProviderKind::from_str(provider)),
            default_provider: ProviderKind::from_str(provider),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
        }
    }

    /// Inserts a dummy PtyClient placeholder into `app.engine.providers` so that the
    /// session appears "active" without actually spawning a process.
    fn mark_active(app: &mut App, session_id: &str) {
        let client =
            crate::pty::PtyClient::spawn("echo", &[], std::path::Path::new("/tmp"), 24, 80, 1000)
                .expect("spawn echo for test");
        app.engine.providers.insert(session_id.to_string(), client);
    }

    fn dummy_changed_file(path: &str) -> dux_core::model::ChangedFile {
        dux_core::model::ChangedFile {
            status: "M".to_string(),
            path: path.to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
        }
    }

    /// Adding a new (agent-less) project selects it; the right-pane changed-files
    /// lists must be cleared so the previously selected project's modified files
    /// don't appear to belong to the brand-new project.
    #[test]
    fn adding_project_clears_stale_changed_files() {
        let session = make_session("s1", "claude", "/tmp/wt/a");
        let existing = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![existing]);

        app.engine.staged_files = vec![dummy_changed_file("staged.rs")];
        app.engine.unstaged_files = vec![dummy_changed_file("a.rs"), dummy_changed_file("b.rs")];

        // The engine worker has already added the project to engine state;
        // applying the outcome selects it and must refresh the file lists.
        let new_project = make_project("project-2", "claude");
        app.engine.projects.push(new_project);
        app.apply_project_persistence_outcome(ProjectPersistenceOutcome {
            action: ProjectPersistenceAction::Add {
                project: make_project("project-2", "claude"),
                status_message: "Added project".to_string(),
            },
            view: ProjectPersistenceView::Added {
                project_id: "project-2".to_string(),
                status_message: "Added project".to_string(),
            },
            status_op_id: None,
        });

        assert!(
            app.selected_session().is_none(),
            "new agent-less project has no selected agent"
        );
        assert!(
            app.engine.staged_files.is_empty(),
            "staged files should be cleared when switching to an agent-less project"
        );
        assert!(
            app.engine.unstaged_files.is_empty(),
            "unstaged files should be cleared when switching to an agent-less project"
        );
    }

    /// Removing a project refreshes the changed-files panel for the new
    /// selection rather than echoing the removed project's stale files.
    #[test]
    fn removing_project_clears_stale_changed_files() {
        let session = make_session("s1", "claude", "/tmp/wt/a");
        let p1 = make_project("project-1", "claude");
        let mut p2 = make_project("project-2", "claude");
        p2.name = "second".to_string();
        let mut app = test_app_with_sessions(vec![session], vec![p1, p2]);
        app.rebuild_left_items();

        app.engine.staged_files = vec![dummy_changed_file("staged.rs")];
        app.engine.unstaged_files = vec![dummy_changed_file("a.rs")];
        app.selected_left = app.left_items().len().saturating_sub(1);

        // Simulate the worker having removed project-2 from engine state.
        app.engine.projects.retain(|p| p.id != "project-2");
        app.apply_project_persistence_outcome(ProjectPersistenceOutcome {
            action: ProjectPersistenceAction::Remove {
                project_id: "project-2".to_string(),
                project_name: "second".to_string(),
            },
            view: ProjectPersistenceView::Removed {
                project_name: "second".to_string(),
            },
            status_op_id: None,
        });

        assert!(
            app.engine.staged_files.is_empty(),
            "staged files should be cleared after removing a project"
        );
        assert!(
            app.engine.unstaged_files.is_empty() || app.selected_session().is_some(),
            "unstaged files should reflect the new selection after removing a project"
        );
    }

    /// Removing an agent-less project contributes zero rows to the flat list,
    /// so the cursor must not move. The old `saturating_sub(1)` jostled the
    /// selection up one even though the removed project had no rows to reclaim.
    #[test]
    fn removing_agentless_project_leaves_selection_put() {
        let p1 = make_project("project-1", "codex");
        let mut p2 = make_project("project-2", "codex");
        p2.name = "empty".to_string();
        let mut sessions = Vec::new();
        for id in ["s1", "s2", "s3"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Active;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![p1, p2]);
        app.rebuild_left_items();

        // Select an agent below the top of the list.
        app.selected_left = 1;
        let selected_id = app.selected_session().map(|s| s.id.clone());
        assert_eq!(selected_id.as_deref(), Some("s2"));

        // Worker removed the agent-less project from engine state.
        app.engine.projects.retain(|p| p.id != "project-2");
        app.apply_project_persistence_outcome(ProjectPersistenceOutcome {
            action: ProjectPersistenceAction::Remove {
                project_id: "project-2".to_string(),
                project_name: "empty".to_string(),
            },
            view: ProjectPersistenceView::Removed {
                project_name: "empty".to_string(),
            },
            status_op_id: None,
        });

        assert_eq!(
            app.selected_left, 1,
            "removing an agent-less project must not move the cursor",
        );
        assert_eq!(
            app.selected_session().map(|s| s.id.clone()).as_deref(),
            Some("s2"),
            "the same agent should still be selected",
        );
    }

    /// Reloading config in the flat model must preserve the agent selection.
    /// The old clamp against `engine.projects.len()` was meaningless (the flat
    /// list indexes agent rows, not projects) and forced the cursor to the top.
    #[test]
    fn config_reload_preserves_agent_selection() {
        let project = make_project_at("project-1", "codex", "/tmp/project");
        let mut sessions = Vec::new();
        for id in ["s1", "s2", "s3"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Active;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![project.clone()]);
        // Persist the project so the reload path (which reloads projects from the
        // store) keeps it instead of wiping it.
        app.engine
            .session_store
            .upsert_project(&crate::config::ProjectConfig {
                id: project.id.clone(),
                path: project.path.clone(),
                name: Some(project.name.clone()),
                default_provider: None,
                leading_branch: project.leading_branch.clone(),
                auto_reopen_agents: project.auto_reopen_agents,
                startup_command: project.startup_command.clone(),
                env: project.env.clone(),
            })
            .expect("seed project into store");
        app.rebuild_left_items();

        app.selected_left = 2;
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("s3"));

        let config = app.engine.config.clone();
        app.apply_reloaded_config(config).expect("reload config");

        assert_eq!(
            app.selected_left, 2,
            "config reload must not reset the agent selection to the top",
        );
        assert_eq!(
            app.selected_session().map(|s| s.id.as_str()),
            Some("s3"),
            "the same agent must stay selected across a config reload",
        );
    }

    /// The "Inactive (N)" toggle count must reflect the active search filter:
    /// it counts only inactive agents currently visible, not every inactive
    /// session regardless of the query.
    #[test]
    fn inactive_toggle_count_honors_the_active_filter() {
        let project = make_project_at("project-1", "codex", "/tmp/project");
        let mut sessions = Vec::new();
        // One active agent (excluded from the inactive tail regardless).
        let mut active = make_session("keep-active", "codex", "/tmp/worktree-a");
        active
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        active.status = SessionStatus::Active;
        sessions.push(active);
        // Three inactive agents: two match the "keep" query, one does not.
        for id in ["keep-1", "keep-2", "drop-1"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Detached;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![project]);

        // No filter: all three inactive agents count.
        assert_eq!(app.visible_inactive_count(), 3);

        // Filter to "keep": only the two matching inactive agents remain visible.
        app.agent_filter = Some(TextInput::with_text("keep".to_string()));
        app.rebuild_left_items();
        assert_eq!(
            app.visible_inactive_count(),
            2,
            "the toggle count must drop the filtered-out inactive agent",
        );
    }

    /// The `manage-projects` target is one-and-done: a project-scoped action
    /// consumes it, so a second action falls back to the ordinary selection.
    #[test]
    fn project_action_consumes_manage_projects_target() {
        let p1 = make_project("project-1", "codex");
        let mut p2 = make_project("project-2", "codex");
        p2.name = "empty".to_string();
        let mut sessions = Vec::new();
        for id in ["s1", "s2"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Active;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![p1, p2]);
        app.rebuild_left_items();
        app.selected_left = 0;

        // Point the chooser at the agent-less project and run one project action.
        app.project_chooser_context = Some("project-2".to_string());
        app.open_configure_project_env().expect("configure env");

        // The action captured project-2 (proof it resolved the target)…
        match &app.prompt {
            PromptState::ConfigureProjectEnv { project_id, .. } => {
                assert_eq!(project_id, "project-2");
            }
            other => panic!("expected ConfigureProjectEnv prompt, got {other:?}"),
        }
        // …and the target is now consumed so the next action won't reuse it.
        assert!(
            app.project_chooser_context.is_none(),
            "the manage-projects target must be cleared after one project action",
        );
    }

    /// Confirming the project chooser after the picked project vanished from
    /// `engine.projects` must report an error and close the prompt, not panic.
    #[test]
    fn confirm_project_chooser_selection_handles_vanished_project() {
        let project = make_project("project-1", "codex");
        let mut app = test_app_with_sessions(vec![], vec![project]);
        app.prompt = PromptState::PickProject {
            intent: ProjectChooserIntent::NewAgent,
            entries: vec![ProjectChooserEntry {
                id: "ghost".to_string(),
                name: "ghost".to_string(),
                path: "/tmp/ghost".to_string(),
                agent_count: 0,
                path_missing: false,
            }],
            list: SearchableList::new(),
        };

        app.confirm_project_chooser_selection()
            .expect("must not panic when the project is gone");

        assert!(
            matches!(app.prompt, PromptState::None),
            "the prompt must close",
        );
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
        assert!(app.status.text().contains("no longer available"));
    }

    #[test]
    fn project_chooser_search_filters_then_confirms_the_visible_pick() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let press = |app: &mut App, code: KeyCode| {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
                .unwrap();
        };

        let mut p1 = make_project("alpha", "codex");
        p1.name = "alpha".to_string();
        let mut p2 = make_project("beta", "codex");
        p2.name = "beta".to_string();
        let mut p3 = make_project("gamma", "codex");
        p3.name = "gamma".to_string();
        let mut app = test_app_with_sessions(vec![], vec![p1, p2, p3]);

        app.open_project_chooser(ProjectChooserIntent::Manage)
            .unwrap();

        // `/` enters search; typing "beta" narrows the visible list to one row.
        press(&mut app, KeyCode::Char('/'));
        for c in "beta".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        // Commit the query (leave search mode), then confirm the sole match.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);

        // Manage intent records the picked project as the action target: even
        // though "beta" is index 1 in `entries`, the visible-index resolution
        // must land on it, not on `entries[0]`.
        assert_eq!(app.project_chooser_context.as_deref(), Some("beta"));
    }

    #[test]
    fn detach_finds_conflict_on_same_worktree() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app
            .engine
            .detach_conflicting_worktree_session("/tmp/wt/a", "s2")
            .map(|d| d.label);
        assert!(label.is_some());
        assert!(!app.engine.providers.contains_key("s1"));
    }

    #[test]
    fn detach_no_conflict_different_path() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/b");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app
            .engine
            .detach_conflicting_worktree_session("/tmp/wt/b", "s2")
            .map(|d| d.label);
        assert!(label.is_none());
        assert!(app.engine.providers.contains_key("s1"));
    }

    #[test]
    fn detach_excludes_self() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);
        mark_active(&mut app, "s1");

        let label = app
            .engine
            .detach_conflicting_worktree_session("/tmp/wt/a", "s1")
            .map(|d| d.label);
        assert!(label.is_none());
        assert!(app.engine.providers.contains_key("s1"));
    }

    #[test]
    fn detach_conflicting_worktree_session_removes_pty() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "codex");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app
            .engine
            .detach_conflicting_worktree_session("/tmp/wt/a", "s2")
            .map(|d| d.label);
        assert!(label.is_some());
        assert!(!app.engine.providers.contains_key("s1"));
        let s1_session = app.engine.sessions.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s1_session.status, SessionStatus::Detached);
    }

    #[test]
    fn detach_conflicting_returns_none_when_no_conflict() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let label = app
            .engine
            .detach_conflicting_worktree_session("/tmp/wt/a", "s1")
            .map(|d| d.label);
        assert!(label.is_none());
    }

    #[test]
    fn delete_session_preserves_shared_worktree() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![s1, s2], vec![project]);

        // Deleting s1 should preserve the worktree because s2 still uses it.
        // We can't call do_delete_session directly because git::remove_worktree
        // would fail on a non-existent repo, but we can verify the guard logic.
        let has_sibling = app.engine.sessions.iter().any(|s| {
            s.id != "s1" && s.managed_worktree().expect("managed test session") == "/tmp/wt/a"
        });
        assert!(has_sibling, "sibling session should exist");
    }

    #[test]
    fn delete_session_allows_removal_when_last() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![s1], vec![project]);

        let has_sibling = app.engine.sessions.iter().any(|s| {
            s.id != "s1" && s.managed_worktree().expect("managed test session") == "/tmp/wt/a"
        });
        assert!(!has_sibling, "no sibling session should exist");
    }

    #[test]
    fn should_resume_only_for_providers_started_on_session() {
        let mut session = make_session("s1", "claude", "/tmp/wt/a");
        session.started_providers = vec!["claude".to_string()];
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session.clone()], vec![project]);

        assert!(app.engine.should_resume_session(&session));

        app.engine.sessions[0].provider = ProviderKind::from_str("codex");
        let session = app.engine.sessions[0].clone();
        assert!(!app.engine.should_resume_session(&session));

        app.engine.sessions[0]
            .started_providers
            .push("codex".to_string());
        let session = app.engine.sessions[0].clone();
        assert!(app.engine.should_resume_session(&session));
    }

    #[test]
    fn mark_session_provider_started_persists_history() {
        let session = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![project]);

        app.engine
            .mark_session_provider_started("s1", &dux_core::model::ProviderKind::new("claude"));

        assert_eq!(
            app.engine.sessions[0].started_providers,
            vec!["claude".to_string()]
        );
        let persisted = app
            .engine
            .session_store
            .load_sessions()
            .expect("load sessions");
        assert_eq!(persisted[0].started_providers, vec!["claude".to_string()]);
    }

    /// Build a `Project` whose `path` points at a caller-controlled directory,
    /// so tests can decide whether git operations succeed or fail.
    fn make_project_at(id: &str, provider: &str, path: &str) -> Project {
        Project {
            id: id.to_string(),
            name: "demo".to_string(),
            path: path.to_string(),
            explicit_default_provider: Some(ProviderKind::from_str(provider)),
            default_provider: ProviderKind::from_str(provider),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
            created_at: None,
        }
    }

    /// With `delete_worktree = false`, the session record is removed but the
    /// worktree on disk is left alone and git is never invoked. The project
    /// path here is not a git repo — if the code accidentally invoked git it
    /// would return `Err` and this test would catch it.
    #[test]
    fn do_delete_session_preserves_worktree_when_flag_off() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.do_delete_session("s1", false)
            .expect("delete should succeed without touching git");

        assert!(
            app.engine.sessions.iter().all(|s| s.id != "s1"),
            "session should be removed"
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory must be preserved on disk when delete_worktree=false",
        );
    }

    /// Deleting the selected agent must land the cursor on the row that slid
    /// into the freed slot (id-stable reselection), not on the row above it.
    /// The old `saturating_sub(1)` double-adjusted after the rebuild already
    /// re-clamped, jumping the cursor up one row.
    #[test]
    fn delete_selected_agent_keeps_cursor_on_next_row() {
        let project = make_project_at("project-1", "codex", "/tmp/project");
        let mut sessions = Vec::new();
        for id in ["s1", "s2", "s3"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Active;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![project]);
        app.rebuild_left_items();

        // Select the middle session (display index 1 == s2).
        app.selected_left = 1;
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("s2"));

        app.do_delete_session("s2", false).expect("delete s2");

        // The cursor stays at display index 1, which now holds s3 (the row that
        // slid up), NOT s1 (which a decrement would have selected).
        assert_eq!(
            app.selected_session().map(|s| s.id.as_str()),
            Some("s3"),
            "cursor should land on the row that took the deleted row's place",
        );
    }

    /// Deleting a row OTHER than the selected one must not drag the selection
    /// off the still-present selected agent.
    #[test]
    fn delete_unselected_agent_leaves_selection_put() {
        let project = make_project_at("project-1", "codex", "/tmp/project");
        let mut sessions = Vec::new();
        for id in ["s1", "s2", "s3"] {
            let mut s = make_session(id, "codex", &format!("/tmp/worktree-{id}"));
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            s.status = SessionStatus::Active;
            sessions.push(s);
        }
        let mut app = test_app_with_sessions(sessions, vec![project]);
        app.rebuild_left_items();

        // Select the first session, then delete the middle (unselected) one.
        app.selected_left = 0;
        assert_eq!(app.selected_session().map(|s| s.id.as_str()), Some("s1"));

        app.do_delete_session("s2", false).expect("delete s2");

        assert_eq!(
            app.selected_session().map(|s| s.id.as_str()),
            Some("s1"),
            "deleting a different row must not move the selection",
        );
    }

    /// When another session shares the worktree, the worktree must be
    /// preserved even if the user checked "also delete the worktree" — other
    /// sessions still depend on it. Git must not be invoked.
    #[test]
    fn do_delete_session_keeps_shared_worktree_even_when_flag_on() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        let mut s2 = make_session("s2", "codex", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        s2.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);

        app.do_delete_session("s1", true)
            .expect("delete should succeed without touching git for shared worktree");

        assert!(
            app.engine.sessions.iter().all(|s| s.id != "s1"),
            "s1 should be removed"
        );
        assert!(
            app.engine.sessions.iter().any(|s| s.id == "s2"),
            "s2 should remain"
        );
        assert!(
            worktree_dir.path().exists(),
            "shared worktree must be preserved when siblings exist",
        );
    }

    /// If git fails to remove the worktree, the session record must remain —
    /// otherwise the user loses their agent with no way to retry. We force
    /// the git call to fail by pointing the project path at a directory that
    /// is not a git repository.
    #[test]
    fn do_delete_session_preserves_session_when_git_fails() {
        let project_dir = tempdir().expect("project tempdir");
        // Intentionally NOT a git repo — `git worktree remove` will exit
        // non-zero, which bubbles up as Err from git::remove_worktree.
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let err = app
            .do_delete_session("s1", true)
            .expect_err("git should fail against a non-git project dir");
        let msg = format!("{err:#}");
        assert!(
            msg.to_lowercase().contains("worktree") || msg.contains("git"),
            "error should mention git/worktree, got: {msg}",
        );

        assert!(
            app.engine.sessions.iter().any(|s| s.id == "s1"),
            "session must be preserved when git fails so user can retry",
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory should be untouched on failure",
        );
    }

    /// Graceful delete vanishes the session immediately: its PTY is SIGTERMed and
    /// held for a background reap, and the worktree is removed in the background
    /// only after the agent exits. The session no longer lingers until the worker
    /// reports — the user-chosen tradeoff for a snappy, non-blocking delete.
    #[test]
    fn begin_delete_session_vanishes_session_immediately() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);

        assert!(
            app.engine.sessions.iter().all(|s| s.id != "s1"),
            "the session vanishes from the UI at once, not after the worktree removal",
        );
    }

    /// When the async path does NOT need to run git (no siblings + flag off),
    /// cleanup is safe to run inline and the session should be gone by the
    /// time `begin_delete_session` returns.
    #[test]
    fn begin_delete_session_completes_inline_when_no_git_needed() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", false);

        assert!(
            app.engine.sessions.iter().all(|s| s.id != "s1"),
            "no-git path should complete immediately",
        );
        assert!(
            worktree_dir.path().exists(),
            "worktree directory must be preserved when the flag is off",
        );
    }

    /// `finish_delete_session` is the handler invoked both inline and from
    /// the worker event. It must be idempotent: if the session has already
    /// been removed (e.g. a duplicate worker event) it should no-op.
    #[test]
    fn finish_delete_session_is_idempotent() {
        let mut s1 = make_session("s1", "claude", "/tmp/wt/a");
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.finish_delete_session("s1", WorktreeRemoval::PreservedOrphan, true)
            .expect("first finish succeeds");
        // Second call must not panic or return Err even though session is gone.
        app.finish_delete_session("s1", WorktreeRemoval::PreservedOrphan, true)
            .expect("second finish is a no-op");
    }

    /// Deleting a session must clear its PTY-activity entry (now owned by the
    /// engine) so a stale timestamp can't keep a deleted agent "working".
    #[test]
    fn finish_delete_session_clears_pty_activity_entry() {
        let mut s1 = make_session("s1", "claude", "/tmp/wt/a");
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.engine
            .pty_activity
            .insert("s1".to_string(), std::time::Instant::now());
        app.engine
            .pty_input
            .insert("s1".to_string(), std::time::Instant::now());
        assert!(app.engine.pty_activity.contains_key("s1"));
        assert!(app.engine.pty_input.contains_key("s1"));

        app.finish_delete_session("s1", WorktreeRemoval::PreservedOrphan, true)
            .expect("finish succeeds");

        assert!(
            !app.engine.pty_activity.contains_key("s1"),
            "deleting a session must drop its pty_activity entry",
        );
        assert!(
            !app.engine.pty_input.contains_key("s1"),
            "deleting a session must drop its pty_input entry",
        );
    }

    /// Kicking off the async delete path should mark the session as
    /// pending so the UI can dim the row.
    #[test]
    fn begin_delete_session_tracks_pending_deletion() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);

        assert!(
            app.engine.pending_deletions.contains("s1"),
            "session must be marked pending while async worker runs",
        );
    }

    /// The inline (no-git) path completes immediately, so pending_deletions
    /// should never gain the session in the first place.
    #[test]
    fn begin_delete_session_inline_does_not_track() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", false);

        assert!(
            app.engine.pending_deletions.is_empty(),
            "inline path should never populate pending_deletions",
        );
    }

    /// A second delete request for a session that's already being deleted
    /// must be refused with an error, and must NOT spawn another worker
    /// (i.e. the pending-deletions set size stays at 1).
    #[test]
    fn begin_delete_session_rejects_duplicate_request() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);
        assert_eq!(
            app.engine.pending_deletions.len(),
            1,
            "first call records pending"
        );

        app.begin_delete_session("s1", true);
        assert_eq!(
            app.engine.pending_deletions.len(),
            1,
            "duplicate request must not spawn a second worker",
        );
    }

    /// If the session was removed by another code path while the async
    /// delete worker was running, the worker's completion event must still
    /// overwrite the Busy status line when the message matches.
    #[test]
    fn worktree_remove_completed_clears_busy_when_session_already_gone() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        // Simulate the Busy state set by `begin_delete_session`, including the
        // keyed status op stashed in `pending_delete_ops`.
        let busy_msg = "Removing worktree for agent \"branch-s1\"\u{2026}";
        let op = app.build_delete_status_op("s1", busy_msg.to_string());
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_delete_ops.insert("s1".to_string(), op);
        app.engine.pending_deletions.insert("s1".to_string());

        // Another code path removes the session before the worker replies.
        app.engine.sessions.retain(|s| s.id != "s1");

        // The worker then reports success.
        app.engine
            .worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(dux_core::engine::RemovedBranches::Deleted(
                    dux_core::git::RemoveResult::default(),
                )),
            })
            .expect("channel send");
        app.drain_events();

        assert!(
            app.engine.pending_deletions.is_empty(),
            "pending guard must be cleared on completion",
        );
        assert_ne!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "Busy status must not linger after worker completes, got: {}",
            app.status.text(),
        );
    }

    /// When the session is already gone AND the status line has already been
    /// overwritten by a later Info action (e.g. project deletion), the
    /// worker completion should not clobber the newer message.
    #[test]
    fn worktree_remove_completed_does_not_clobber_newer_info() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let op = app.build_delete_status_op("s1", "Removing worktree\u{2026}".to_string());
        app.pending_delete_ops.insert("s1".to_string(), op);
        app.engine.pending_deletions.insert("s1".to_string());
        app.engine.sessions.retain(|s| s.id != "s1");

        // Another action already set a non-Busy status.
        app.set_info("Deleted project \"demo\" and all its agents");

        app.engine
            .worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(dux_core::engine::RemovedBranches::Deleted(
                    dux_core::git::RemoveResult::default(),
                )),
            })
            .expect("channel send");
        app.drain_events();

        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Info,
            "tone should remain Info",
        );
        assert!(
            app.status.text().contains("Deleted project"),
            "the project-deletion message must not be clobbered, got: {}",
            app.status.text(),
        );
    }

    /// When the session is already gone AND the status line shows a Busy
    /// message from an *unrelated* operation (push, pull, etc.), the worker
    /// completion should not clobber it — the message text doesn't match
    /// ours, even though the tone is also Busy.
    #[test]
    fn worktree_remove_completed_does_not_clobber_unrelated_busy() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let op = app.build_delete_status_op(
            "s1",
            "Removing worktree for agent \"branch-s1\"\u{2026}".to_string(),
        );
        app.pending_delete_ops.insert("s1".to_string(), op);
        app.engine.pending_deletions.insert("s1".to_string());
        app.engine.sessions.retain(|s| s.id != "s1");

        // An unrelated operation set its own Busy message.
        app.set_busy("Pushing to remote\u{2026}");

        app.engine
            .worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Ok(dux_core::engine::RemovedBranches::Deleted(
                    dux_core::git::RemoveResult::default(),
                )),
            })
            .expect("channel send");
        app.drain_events();

        // The status should still show the push Busy, not "Worktree removal
        // finished."
        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Busy,
            "tone should remain Busy from the push",
        );
        assert_eq!(
            app.status.message(),
            "Pushing to remote\u{2026}",
            "the push message must not be clobbered, got: {}",
            app.status.message(),
        );
    }

    /// Project deletion must be refused when any of the project's sessions
    /// have an async worktree removal in-flight. Allowing it would race the
    /// synchronous `do_delete_session` against the worker and could leave the
    /// project half-deleted with an orphaned worktree.
    #[test]
    fn delete_selected_project_blocked_when_pending() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        // Simulate an async delete in-flight for this session.
        app.engine.pending_deletions.insert("s1".to_string());

        // The project is the first item in the list, select it.
        app.selected_left = 0;

        app.delete_selected_project()
            .expect("should return Ok (error reported via status line)");

        // Session must still be present — deletion was refused.
        assert!(
            app.engine.sessions.iter().any(|s| s.id == "s1"),
            "session must not be removed when deletion is blocked",
        );
        assert!(
            app.engine.projects.iter().any(|p| p.id == "project-1"),
            "project must not be removed when deletion is blocked",
        );
        assert_eq!(
            app.status.tone(),
            crate::statusline::StatusTone::Error,
            "should show an error explaining why deletion was blocked",
        );
    }

    #[test]
    fn delete_selected_project_blocked_when_a_tab_is_launching() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        // A tab of this project's session has a launch in flight (session-slot tab id ==
        // session id). Deleting the project must be refused up front, not silently
        // skip this session and then falsely claim success.
        app.engine
            .mark_in_flight(dux_core::engine::InFlightKey::AgentLaunch("s1".to_string()));
        app.selected_left = 0;

        app.delete_selected_project()
            .expect("should return Ok (error reported via status line)");

        assert!(
            app.engine.sessions.iter().any(|s| s.id == "s1"),
            "session must not be removed while a tab is launching",
        );
        assert!(
            app.engine.projects.iter().any(|p| p.id == "project-1"),
            "project must not be removed while a tab is launching",
        );
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Error);
    }

    /// When the worker fails to delete a worktree, the error message should
    /// include the agent label so the user knows which one failed.
    #[test]
    fn worktree_remove_failure_identifies_agent() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        let op = app.build_delete_status_op(
            "s1",
            "Removing worktree for agent \"branch-s1\"\u{2026}".to_string(),
        );
        app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
        app.pending_delete_ops.insert("s1".to_string(), op);
        app.engine.pending_deletions.insert("s1".to_string());

        app.engine
            .worker_tx
            .send(WorkerEvent::WorktreeRemoveCompleted {
                session_id: "s1".to_string(),
                result: Err("fatal: not a git repository".to_string()),
            })
            .expect("channel send");
        app.drain_events();

        let msg = app.status.text();
        assert!(
            msg.contains("branch-s1"),
            "error should include the agent's branch name, got: {msg}",
        );
        assert!(
            msg.contains("not a git repository"),
            "error should include the git error, got: {msg}",
        );
    }

    /// The async success path (session still present at completion) now resolves
    /// the keyed delete op rather than letting `apply_finish_delete_session_outcome`
    /// author the line. The wording must stay byte-identical to the legacy path.
    #[test]
    fn async_delete_success_resolves_op_with_exact_wording() {
        for (branch, expected) in [
            (
                dux_core::git::BranchDeletion::Deleted,
                "Deleted claude agent from project \"demo\" with branch \"branch-s1\".",
            ),
            (
                dux_core::git::BranchDeletion::AlreadyGone,
                "Deleted agent (branch \"branch-s1\" was already removed).",
            ),
        ] {
            let mut s1 = make_session("s1", "claude", "/tmp/wt");
            s1.workspace
                .as_managed_mut()
                .expect("managed test session")
                .project_id = "project-1".to_string();
            let project = make_project("project-1", "claude");
            let mut app = test_app_with_sessions(vec![s1], vec![project]);

            let op = app.build_delete_status_op(
                "s1",
                "Removing worktree for agent \"branch-s1\"\u{2026}".to_string(),
            );
            app.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
            app.pending_delete_ops.insert("s1".to_string(), op);
            app.engine.pending_deletions.insert("s1".to_string());

            app.engine
                .worker_tx
                .send(WorkerEvent::WorktreeRemoveCompleted {
                    session_id: "s1".to_string(),
                    result: Ok(dux_core::engine::RemovedBranches::Deleted(
                        dux_core::git::RemoveResult {
                            branch: branch.clone(),
                            initial_branch: None,
                        },
                    )),
                })
                .expect("channel send");
            app.drain_events();

            assert_eq!(app.status.message(), expected, "branch outcome {branch:?}",);
            assert!(
                !app.engine.sessions.iter().any(|s| s.id == "s1"),
                "session should be cleaned up after async success",
            );
            assert!(
                app.pending_delete_ops.is_empty(),
                "the op must be consumed on resolution",
            );
        }
    }

    #[test]
    fn finish_delete_messages_match_each_removal_variant() {
        use dux_core::engine::{FinishDeleteSessionOutcome, WorktreeRemoval};

        let cases = [
            (
                WorktreeRemoval::SkippedForSiblings,
                "Deleted claude agent \"branch-s1\". Worktree preserved because other sessions still use it.",
            ),
            (
                WorktreeRemoval::PreservedShared,
                "Deleted claude session for agent \"branch-s1\". Worktree preserved for remaining sessions.",
            ),
            (
                WorktreeRemoval::PreservedOrphan,
                "Deleted claude agent \"branch-s1\". Worktree preserved at /tmp/wt.",
            ),
            (
                WorktreeRemoval::Performed {
                    branches: dux_core::engine::RemovedBranches::Deleted(
                        dux_core::git::RemoveResult {
                            branch: dux_core::git::BranchDeletion::AlreadyGone,
                            initial_branch: None,
                        },
                    ),
                },
                "Deleted agent (branch \"branch-s1\" was already removed).",
            ),
            (
                WorktreeRemoval::Performed {
                    branches: dux_core::engine::RemovedBranches::Deleted(
                        dux_core::git::RemoveResult::default(),
                    ),
                },
                "Deleted claude agent from project \"demo\" with branch \"branch-s1\".",
            ),
        ];

        for (removal, expected) in cases {
            let session = make_session("s1", "claude", "/tmp/wt");
            let project = make_project("project-1", "claude");
            let mut app = test_app_with_sessions(vec![session.clone()], vec![project.clone()]);
            let outcome = FinishDeleteSessionOutcome {
                session,
                project: Some(project),
                other_sessions_on_worktree: matches!(
                    removal,
                    WorktreeRemoval::SkippedForSiblings | WorktreeRemoval::PreservedShared
                ),
                project_still_has_sessions: false,
            };
            app.apply_finish_delete_session_outcome("s1", outcome, removal.clone(), true);
            assert_eq!(app.status.message(), expected, "variant {removal:?}");
        }
    }

    /// A drifted agent loses TWO branches, so the status line must name the
    /// second one. Saying only "with branch <current>" is not a lie by itself,
    /// but it leaves the user unaware that their original branch is gone.
    #[test]
    fn delete_status_names_the_branch_the_agent_was_born_on_when_it_drifted() {
        let mut session = make_session("s1", "claude", "/tmp/wt");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "born-here".to_string();
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session.clone()], vec![project.clone()]);
        let outcome = FinishDeleteSessionOutcome {
            session,
            project: Some(project),
            other_sessions_on_worktree: false,
            project_still_has_sessions: false,
        };

        app.apply_finish_delete_session_outcome(
            "s1",
            outcome,
            WorktreeRemoval::Performed {
                branches: dux_core::engine::RemovedBranches::Deleted(dux_core::git::RemoveResult {
                    branch: dux_core::git::BranchDeletion::Deleted,
                    initial_branch: Some(dux_core::git::BranchDeletion::Deleted),
                }),
            },
            true,
        );

        assert_eq!(
            app.status.message(),
            "Deleted claude agent from project \"demo\" with branch \"branch-s1\". \
             Its original branch \"born-here\" was deleted too."
        );
    }

    /// The keep path on the TUI status line: nothing was deleted, so the line
    /// must not claim a deletion. It names the kept branches, why each stayed,
    /// and the manual way to remove one.
    #[test]
    fn delete_status_says_which_branches_were_kept_and_why() {
        let mut session = make_session("s1", "claude", "/tmp/wt");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "develop".to_string();
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = dux_core::model::BranchProvenance::AttachedExisting;
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session.clone()], vec![project.clone()]);
        let outcome = FinishDeleteSessionOutcome {
            session,
            project: Some(project),
            other_sessions_on_worktree: false,
            project_still_has_sessions: false,
        };

        app.apply_finish_delete_session_outcome(
            "s1",
            outcome,
            WorktreeRemoval::Performed {
                branches: dux_core::engine::RemovedBranches::Kept(
                    dux_core::model::BranchProvenance::AttachedExisting,
                ),
            },
            true,
        );

        assert_eq!(
            app.status.message(),
            "Deleted claude agent \"branch-s1\" and removed its worktree. Its branch \
             \"branch-s1\" was created inside this agent's worktree and was kept, and its \
             branch \"develop\" existed before this agent and was kept. Delete either \
             yourself with git branch -D \"branch-s1\" or git branch -D \"develop\" if you \
             no longer need them."
        );
    }

    /// The same wording arrives through the ASYNC path, whose resolver captured
    /// the session's facts at dispatch time.
    #[test]
    fn the_async_delete_op_reports_kept_branches_too() {
        let mut session = make_session("s1", "claude", "/tmp/wt");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = dux_core::model::BranchProvenance::Adopted;
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![session], vec![project]);

        let op = app.build_delete_status_op("s1", "Removing worktree\u{2026}".to_string());
        let reaction = op
            .resolve(&TuiDeleteOutcome::SucceededPresent {
                branches: dux_core::engine::RemovedBranches::Kept(
                    dux_core::model::BranchProvenance::Adopted,
                ),
            })
            .into_reaction();
        let dux_core::engine::EventReaction::Status(status) = reaction else {
            panic!("the op must resolve to a status");
        };
        assert_eq!(
            status.message,
            "Deleted claude agent \"branch-s1\" and removed its worktree. Its branch \
             \"branch-s1\" came with the worktree this agent adopted and was kept. Delete \
             it yourself with git branch -D \"branch-s1\" if you no longer need it."
        );
    }

    #[test]
    fn kill_runtime_targets_agent_clears_in_flight_launch_key() {
        // G3 regression: the Agent branch of `kill_runtime_targets` used to
        // hand-roll the tab-runtime clear and missed the in-flight
        // `AgentLaunch` key, leaving a stale marker that made a later
        // `DispatchAgentLaunch` report "already launching" forever. Now
        // routed through the shared `clear_tab_runtime`.
        let session = make_session("s1", "claude", "/tmp/wt");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![project]);
        mark_active(&mut app, "s1");
        app.engine
            .mark_in_flight(dux_core::engine::InFlightKey::AgentLaunch("s1".to_string()));

        let (killed_agents, _killed_terminals) =
            app.kill_runtime_targets(&[RuntimeTargetId::Agent("s1".to_string())]);

        assert_eq!(killed_agents, 1);
        assert!(
            !app.engine.providers.contains_key("s1"),
            "provider must be dropped"
        );
        assert!(
            !app.engine
                .is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(
                    "s1".to_string()
                )),
            "killing the agent must clear its in-flight AgentLaunch key"
        );
    }

    #[test]
    fn kill_runtime_targets_tab_clears_in_flight_launch_key() {
        // Same G3 regression as the Agent branch above, for an extra tab.
        let session = make_session("s1", "claude", "/tmp/wt");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![project]);
        let tab = dux_core::model::AgentTab {
            id: "tab-1".to_string(),
            session_id: "s1".to_string(),
            provider: ProviderKind::from_str("codex"),
            sort_order: 0,
            created_at: Utc::now(),
        };
        app.engine.agent_tabs.insert(tab.id.clone(), tab);
        mark_active(&mut app, "tab-1");
        app.engine
            .mark_in_flight(dux_core::engine::InFlightKey::AgentLaunch(
                "tab-1".to_string(),
            ));

        let (killed_agents, _killed_terminals) =
            app.kill_runtime_targets(&[RuntimeTargetId::Tab("tab-1".to_string())]);

        assert_eq!(killed_agents, 1);
        assert!(
            !app.engine.providers.contains_key("tab-1"),
            "provider must be dropped"
        );
        assert!(
            !app.engine
                .is_in_flight(&dux_core::engine::InFlightKey::AgentLaunch(
                    "tab-1".to_string()
                )),
            "killing the tab must clear its in-flight AgentLaunch key"
        );
    }

    #[test]
    fn force_reconnect_agent_clears_in_flight_launch_key() {
        // G3 regression: `force_reconnect_agent` used to hand-roll the
        // tab-runtime clear and missed the in-flight `AgentLaunch` key, so a
        // stale marker from a prior launch would make the relaunch dispatch
        // refuse with "already launching". Now routed through
        // `clear_tab_runtime`, so the relaunch proceeds.
        let mut session = make_session("s1", "claude", "");
        let wt = tempdir().expect("worktree tempdir");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = wt.path().to_string_lossy().to_string();
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![session], vec![project]);
        app.rebuild_left_items();
        app.selected_left = 1;
        app.engine
            .mark_in_flight(dux_core::engine::InFlightKey::AgentLaunch("s1".to_string()));

        app.force_reconnect_agent().expect("force reconnect");

        assert!(
            app.status.message().contains("Starting fresh agent"),
            "force reconnect should have dispatched instead of refusing as \
             already-launching: {}",
            app.status.message()
        );
    }

    // ---- terminal_items sort-mode coverage (Phase 4b) --------------------------
    //
    // These mirror the agent-list sort tests above but over the flat Terminals
    // section, and must stay in lockstep with the web `sortFlatTerminals` tests.

    /// Insert a companion terminal with fully controlled sort keys. Spawns a cheap
    /// throwaway PTY (`echo`) for `client`; `terminal_items` never reads it.
    fn insert_test_terminal(
        app: &mut App,
        id: &str,
        sort_order: u64,
        created_at: chrono::DateTime<Utc>,
        label: &str,
        foreground_cmd: Option<&str>,
    ) {
        let client =
            crate::pty::PtyClient::spawn("echo", &[], std::path::Path::new("/tmp"), 24, 80, 1000)
                .expect("spawn echo for test terminal");
        app.engine.companion_terminals.insert(
            id.to_string(),
            CompanionTerminal {
                owner: TerminalOwner::Session("s1".to_string()),
                label: label.to_string(),
                foreground_cmd: foreground_cmd.map(|s| s.to_string()),
                client,
                sort_order,
                created_at,
            },
        );
    }

    fn app_with_one_session() -> App {
        let session = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        test_app_with_sessions(vec![session], vec![project])
    }

    fn terminal_order(app: &App) -> Vec<String> {
        app.terminal_items()
            .into_iter()
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[test]
    fn terminal_items_manual_orders_by_sort_order() {
        let mut app = app_with_one_session();
        app.engine.config.ui.agent_sort = "manual".to_string();
        let now = Utc::now();
        // Insert out of order; sort_order is the sole tiebreaker.
        insert_test_terminal(&mut app, "term-c", 2, now, "zzz", None);
        insert_test_terminal(&mut app, "term-a", 0, now, "mmm", None);
        insert_test_terminal(&mut app, "term-b", 1, now, "aaa", None);

        assert_eq!(terminal_order(&app), vec!["term-a", "term-b", "term-c"]);
    }

    #[test]
    fn terminal_items_created_orders_newest_first() {
        let mut app = app_with_one_session();
        app.engine.config.ui.agent_sort = "created".to_string();
        let t0 = Utc::now();
        insert_test_terminal(&mut app, "term-a", 0, t0, "a", None);
        insert_test_terminal(
            &mut app,
            "term-b",
            1,
            t0 + chrono::Duration::seconds(10),
            "b",
            None,
        );
        insert_test_terminal(
            &mut app,
            "term-c",
            2,
            t0 + chrono::Duration::seconds(20),
            "c",
            None,
        );

        // Newest created first.
        assert_eq!(terminal_order(&app), vec!["term-c", "term-b", "term-a"]);
    }

    #[test]
    fn terminal_items_updated_orders_by_recent_pty_activity() {
        let mut app = app_with_one_session();
        app.engine.config.ui.agent_sort = "updated".to_string();
        let now = Utc::now();
        // Identical created_at so ONLY pty_activity distinguishes them.
        insert_test_terminal(&mut app, "term-a", 0, now, "a", None);
        insert_test_terminal(&mut app, "term-b", 1, now, "b", None);
        insert_test_terminal(&mut app, "term-c", 2, now, "c", None);

        // term-b activity is the most recent (smallest elapsed), term-a the oldest.
        app.engine.pty_activity.insert(
            "term-a".to_string(),
            std::time::Instant::now() - std::time::Duration::from_secs(30),
        );
        app.engine.pty_activity.insert(
            "term-c".to_string(),
            std::time::Instant::now() - std::time::Duration::from_secs(15),
        );
        app.engine
            .pty_activity
            .insert("term-b".to_string(), std::time::Instant::now());

        assert_eq!(terminal_order(&app), vec!["term-b", "term-c", "term-a"]);
    }

    #[test]
    fn terminal_items_name_uses_displayed_label_and_reverses() {
        let mut app = app_with_one_session();
        let now = Utc::now();
        // Displayed name = foreground_cmd when present/non-empty, else label. The
        // sort_order is deliberately anti-alphabetical to prove name wins.
        insert_test_terminal(&mut app, "term-vim", 0, now, "shell", Some("vim"));
        insert_test_terminal(&mut app, "term-bash", 1, now, "bash", None);
        insert_test_terminal(&mut app, "term-htop", 2, now, "shell", Some("htop"));

        app.engine.config.ui.agent_sort = "name".to_string();
        // bash < htop < vim
        assert_eq!(
            terminal_order(&app),
            vec!["term-bash", "term-htop", "term-vim"]
        );

        app.engine.config.ui.agent_sort = "name_desc".to_string();
        assert_eq!(
            terminal_order(&app),
            vec!["term-vim", "term-htop", "term-bash"]
        );
    }

    #[test]
    fn terminal_items_active_floats_working_or_typing_to_top() {
        let mut app = app_with_one_session();
        app.engine.config.ui.agent_sort = "active".to_string();
        let now = Utc::now();
        insert_test_terminal(&mut app, "term-a", 0, now, "a", None);
        insert_test_terminal(&mut app, "term-b", 1, now, "b", None);
        insert_test_terminal(&mut app, "term-c", 2, now, "c", None);
        insert_test_terminal(&mut app, "term-d", 3, now, "d", None);

        // term-c is working (fresh pty_activity, no input); term-b is typing.
        app.engine
            .pty_activity
            .insert("term-c".to_string(), std::time::Instant::now());
        app.engine
            .pty_input
            .insert("term-b".to_string(), std::time::Instant::now());

        // Hot terminals float up keeping base sort_order order (b before c), then
        // the idle rest in base order (a, d).
        assert_eq!(
            terminal_order(&app),
            vec!["term-b", "term-c", "term-a", "term-d"]
        );
    }
}
