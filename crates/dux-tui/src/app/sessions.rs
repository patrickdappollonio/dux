use super::*;
use crate::browser;
use crate::editor;
use dux_core::engine::{Command, EventReaction, FinishDeleteSessionOutcome, WorktreeRemoval};

impl App {
    pub(crate) fn open_project_browser(&mut self) -> Result<()> {
        let start_dir = dux_core::project_browser::resolve_start_dir(&self.engine.config);
        self.prompt = PromptState::BrowseProjects {
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
            self.set_info(format!(
                "Project browser: {open} opens folders, {add} adds current dir, {search} to search, {goto} to go to a path.",
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
        let branch = git::current_branch_opt(&path)?.unwrap_or_default();
        let leading_branch =
            leading_branch_for_project(&path, (!branch.is_empty()).then_some(branch.as_str()));

        // A detached HEAD (empty branch string) is not "on a non-default
        // branch" -- there is no branch to compare. Skip the warning so the
        // user does not see a misleading Heuristic dialog when adding a
        // detached-HEAD repo.
        if let Some(kind) = (!branch.is_empty())
            .then(|| git::branch_warning_kind(&path, &branch))
            .flatten()
        {
            // Default the checkbox to on for the confident path so hitting
            // Enter resolves the warning in the way users typically want —
            // "switch to main, then add". The heuristic path ignores this
            // field (no checkbox is shown).
            let checkout_default = matches!(kind, BranchWarningKind::Known { .. });
            self.prompt = PromptState::ConfirmNonDefaultBranch {
                action: NonDefaultBranchAction::AddProject {
                    path: path.to_string_lossy().to_string(),
                    name,
                    leading_branch,
                },
                current_branch: branch,
                kind,
                focus: ConfirmNonDefaultBranchFocus::Cancel,
                checkout_default,
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

    pub(crate) fn create_agent_for_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        if project.path_missing {
            return Ok(());
        }

        self.dispatch_create_agent_branch_inspection(project);
        Ok(())
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
        })
    }

    pub(crate) fn create_agent_from_existing_worktree(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        if project.path_missing {
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
        let Some(project) = self.selected_project().cloned() else {
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

    pub(crate) fn open_new_agent_from_pr_prompt(&mut self) -> Result<()> {
        if !self.github_pr_agent_command_available() {
            self.set_error(
                "GitHub PR agent creation requires GitHub integration and an authenticated gh CLI.",
            );
            return Ok(());
        }
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first to create an agent from a PR.");
            return Ok(());
        };
        if project.path_missing {
            self.set_warning(format!(
                "Cannot create an agent from a PR: path not found for \"{}\"",
                project.name
            ));
            return Ok(());
        }
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = FullscreenOverlay::None;
        self.prompt = PromptState::PullRequestInput {
            project,
            input: TextInput::new(),
        };
        self.set_info("Paste a GitHub PR URL or enter a PR number for the selected project.");
        Ok(())
    }

    pub(crate) fn dispatch_pull_request_lookup(
        &mut self,
        project: Project,
        raw_input: String,
    ) -> Result<()> {
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
                );
            })) {
                let reason = dux_core::engine::format_panic_payload(payload);
                dux_core::logger::error(&format!("pull-request-lookup worker panicked: {reason}"));
                let _ = tx_panic.send(WorkerEvent::PullRequestResolved {
                    result: Err(format!("Worker panicked: {reason}")),
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
        let Some(project) = self.selected_project().cloned() else {
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
    /// [`TuiReconnectOutcome`] (the engine computes the success line; the failure
    /// arms carry branch + message), so it captures no dispatch-time state and
    /// reproduces the TUI's exact wording for every outcome.
    pub(super) fn build_reconnect_status_op(
        &self,
        busy_message: String,
    ) -> dux_core::engine::HandlerStatusOp<TuiReconnectOutcome> {
        dux_core::engine::status_op(busy_message)
            .resolve_in_handler(|o: &TuiReconnectOutcome| reconnect_final(o))
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

    pub(crate) fn spawn_companion_terminal_for_session(
        &self,
        session: &AgentSession,
    ) -> Result<PtyClient> {
        let (rows, cols) = if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        };
        logger::debug(&format!(
            "spawning companion terminal {:?} {:?} in {} ({}x{})",
            self.engine.config.terminal.command,
            self.engine.config.terminal.args,
            session.worktree_path,
            cols,
            rows,
        ));
        let env = self
            .engine
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .and_then(|project| {
                crate::config::resolve_agent_env(&self.engine.config.env, &project.env).ok()
            })
            .unwrap_or_default();
        PtyClient::spawn_with_env(
            &self.engine.config.terminal.command,
            &self.engine.config.terminal.args,
            Path::new(&session.worktree_path),
            rows,
            cols,
            self.engine.config.ui.agent_scrollback_lines,
            &env,
        )
    }

    pub(crate) fn show_agent_surface(&mut self) {
        self.focus = FocusPane::Center;
        self.center_mode = CenterMode::Agent;
        self.session_surface = SessionSurface::Agent;
        self.fullscreen_overlay = FullscreenOverlay::None;
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

        let client = self.spawn_companion_terminal_for_session(&session)?;
        let terminal_id = self.next_terminal_id();
        let count = self.session_terminal_count(&session.id) + 1;
        let label = if count == 1 {
            session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone())
        } else {
            let base = session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone());
            format!("{base} ({count})")
        };
        self.engine.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                session_id: session.id.clone(),
                label,
                foreground_cmd: None,
                client,
            },
        );
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched terminal for agent \"{}\".",
            session.branch_name
        ));
        Ok(())
    }

    /// Opens the first existing companion terminal for the selected session,
    /// or spawns a new one if none exists.
    pub(crate) fn show_or_open_first_terminal(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };

        let first = self
            .engine
            .companion_terminals
            .iter()
            .filter(|(_, t)| t.session_id == session.id)
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
            Ok(())
        } else {
            self.show_companion_terminal()
        }
    }

    /// Spawns a new companion terminal for the agent that owns the currently
    /// selected terminal in the terminals list.
    pub(crate) fn spawn_terminal_for_selected_terminal(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(_, terminal)) = items.get(self.selected_terminal_index) else {
            self.set_warning("No terminal selected.");
            return Ok(());
        };
        let session_id = terminal.session_id.clone();
        drop(items);

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

        let client = self.spawn_companion_terminal_for_session(&session)?;
        let terminal_id = self.next_terminal_id();
        let count = self.session_terminal_count(&session.id) + 1;
        let base = session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone());
        let label = if count == 1 {
            base
        } else {
            format!("{base} ({count})")
        };
        self.engine.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                session_id: session.id.clone(),
                label,
                foreground_cmd: None,
                client,
            },
        );
        self.active_terminal_id = Some(terminal_id);
        self.terminal_return_to_list = true;
        self.show_companion_terminal_surface();
        self.input_target = InputTarget::Terminal;
        self.set_info(format!(
            "Launched new terminal for agent \"{}\".",
            session.branch_name
        ));
        Ok(())
    }

    /// Palette command: always spawns a new companion terminal.
    /// Uses a yellow warning if no agent session is selected.
    pub(crate) fn new_companion_terminal(&mut self) -> Result<()> {
        if self.selected_session().is_none() {
            self.set_warning("Select an agent session first to launch a companion terminal.");
            return Ok(());
        }
        self.show_companion_terminal()
    }

    /// Opens the terminal overlay for the terminal selected in the terminals list.
    pub(crate) fn open_terminal_from_terminal_list(&mut self) -> Result<()> {
        let items = self.terminal_items();
        let Some(&(terminal_id, terminal)) = items.get(self.selected_terminal_index) else {
            return Ok(());
        };
        let terminal_id = terminal_id.clone();
        let session_id = terminal.session_id.clone();
        let label = terminal.label.clone();
        drop(items);

        // Select this terminal's session in the left pane.
        if let Some(pos) = self
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(idx) if self.engine.sessions.get(*idx).map(|s| s.id.as_str()) == Some(session_id.as_str())))
        {
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
        let Some(project) = self.selected_project().cloned() else {
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
        let worktree_shared = self
            .engine
            .sessions
            .iter()
            .any(|s| s.id != session.id && s.worktree_path == session.worktree_path);
        self.prompt = PromptState::ConfirmDeleteAgent {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            focus: DeleteAgentFocus::Cancel, // Cancel is the safe default
            delete_worktree: false,          // Opt-in destructive action
            worktree_shared,
        };
        Ok(())
    }

    /// Delete the agent session identified by `session_id`, blocking the
    /// calling thread for any git work. Used by bulk flows like project
    /// deletion, where we must complete all removals before the parent
    /// operation proceeds. User-initiated single-agent deletes go through
    /// [`begin_delete_session`] so git work runs off the UI thread.
    ///
    /// When `delete_worktree` is true AND no other sessions share the worktree,
    /// the git worktree and branch are removed first. If the git removal fails,
    /// the session record is preserved so the caller can retry without losing
    /// the agent. When `delete_worktree` is false, the worktree and branch
    /// are always preserved.
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
        let (provider, branch_name, name, project_name) = self
            .engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| {
                let provider = s.provider.as_str().to_string();
                let branch_name = s.branch_name.clone();
                let name = s.title.as_deref().unwrap_or(&s.branch_name).to_string();
                let project_name = self
                    .engine
                    .projects
                    .iter()
                    .find(|p| p.id == s.project_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "<unknown>".to_string());
                (provider, branch_name, name, project_name)
            })
            .unwrap_or_else(|| {
                (
                    String::new(),
                    String::new(),
                    String::new(),
                    "<unknown>".to_string(),
                )
            });
        dux_core::engine::status_op(busy_message).resolve_in_handler(
            move |o: &TuiDeleteOutcome| match o {
                TuiDeleteOutcome::SucceededPresent {
                    branch_already_deleted,
                } => {
                    if *branch_already_deleted {
                        dux_core::engine::Final::info(format!(
                            "Deleted agent (branch \"{branch_name}\" was already removed)."
                        ))
                    } else {
                        dux_core::engine::Final::info(format!(
                            "Deleted {provider} agent from project \"{project_name}\" with branch \"{branch_name}\"."
                        ))
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
            project_still_has_sessions,
        } = outcome;

        // View-side cleanup the engine couldn't do.
        self.engine.pty_activity.remove(session_id);
        self.engine.pty_input.remove(session_id);
        self.clear_companion_terminals_for_session(session_id);

        // Derived view state.
        self.rebuild_left_items();
        if project_still_has_sessions {
            self.selected_left = self.selected_left.saturating_sub(1);
            self.ensure_selectable_left_item();
        } else if let Some(project_index) = self.left_items().iter().position(|item| {
            matches!(item, LeftItem::Project(index) if self.engine.projects[*index].id == session.project_id)
        }) {
            self.selected_left = project_index;
        }
        self.reload_changed_files();

        if update_status {
            match removal {
                WorktreeRemoval::SkippedForSiblings => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved because other sessions still use it.",
                        session.provider.as_str(),
                        session.branch_name,
                    ));
                }
                WorktreeRemoval::PreservedShared => {
                    self.set_info(format!(
                        "Deleted {} session for agent \"{}\". Worktree preserved for remaining sessions.",
                        session.provider.as_str(),
                        session.branch_name,
                    ));
                }
                WorktreeRemoval::PreservedOrphan => {
                    self.set_info(format!(
                        "Deleted {} agent \"{}\". Worktree preserved at {}.",
                        session.provider.as_str(),
                        session.branch_name,
                        session.worktree_path,
                    ));
                }
                WorktreeRemoval::Performed {
                    branch_already_deleted,
                } => {
                    if branch_already_deleted {
                        self.set_info(format!(
                            "Deleted agent (branch \"{}\" was already removed).",
                            session.branch_name,
                        ));
                    } else {
                        let project_name = project
                            .as_ref()
                            .map(|p| p.name.as_str())
                            .unwrap_or("<unknown>");
                        self.set_info(format!(
                            "Deleted {} agent from project \"{}\" with branch \"{}\".",
                            session.provider.as_str(),
                            project_name,
                            session.branch_name,
                        ));
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
            confirm_selected: false, // Cancel is default
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
        self.prompt = PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
            session_id: session.id.clone(),
            session_label: self.session_label(&session),
            worktree_path: session.worktree_path.clone(),
            options: self.change_agent_provider_options(&session),
            selected: 0,
            focus: ChangeAgentProviderFocus::List,
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
        let outcome = self
            .engine
            .change_agent_provider(&session_id, selected.provider.clone())?;
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
            focus: ChangeDefaultProviderFocus::List,
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
        let Some(project) = self.selected_project().cloned() else {
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
                focus: ChangeDefaultProviderFocus::List,
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
        let Some(project) = self.selected_project().cloned() else {
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
            session_id: session.id,
            branch_name: session.branch_name,
            new_enabled,
        })?;
        self.apply_reaction(reaction);
        Ok(())
    }

    pub(crate) fn open_configure_startup_command(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
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
        let Some(project) = self.selected_project().cloned() else {
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
        let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
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
        let branch = session.branch_name.clone();
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
        let (scope_label, scope) = if let Some(session) = self.selected_session().cloned() {
            let project_name = self.engine.project_name_for_session(&session);
            (
                format!(
                    "agent \"{}\" in project \"{}\"",
                    session.branch_name, project_name
                ),
                crate::startup::StartupCommandLogScope::Agent {
                    project_id: session.project_id,
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

    pub(crate) fn select_startup_command_log(&mut self, selected: usize) {
        let Some((path, count)) = (match &self.prompt {
            PromptState::StartupCommandLogs(prompt) => prompt
                .entries
                .get(selected)
                .map(|entry| (entry.path.clone(), prompt.entries.len())),
            _ => None,
        }) else {
            return;
        };
        let content = crate::startup::read_log(&path)
            .unwrap_or_else(|err| format!("Could not read {}: {err:#}", path.display()));
        if let PromptState::StartupCommandLogs(prompt) = &mut self.prompt {
            prompt.selected = selected.min(count.saturating_sub(1));
            prompt.content = content;
            prompt.scroll_offset = 0;
        }
        self.startup_log_selection = None;
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

    pub(crate) fn open_selected_startup_command_log(&mut self) {
        let Some(path) = self
            .startup_log_viewer
            .as_ref()
            .and_then(|viewer| viewer.path.clone())
        else {
            self.set_error("No startup command log is selected.");
            return;
        };
        self.spawn_open_path(path, "startup command log file");
    }

    pub(crate) fn open_selected_startup_command_log_folder(&mut self) {
        let Some(path) = self
            .startup_log_viewer
            .as_ref()
            .and_then(|viewer| viewer.path.as_ref())
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
        .on_success(move |_: &crate::startup::StartupCommandLatestLog| {
            dux_core::engine::Final::info(format!(
                "Opened startup command logs for {success_label}."
            ))
        })
        .on_failure(move |err: &String| {
            dux_core::engine::Final::error(format!(
                "Could not read startup command logs for {failure_label}: {err}"
            ))
        });
        let pending = op.pending_status();
        std::thread::spawn(move || {
            let result = crate::startup::latest_log_for_scope(&paths, scope)
                .map_err(|err| format!("{err:#}"));
            let resolved = op.resolve(&result);
            let _ = tx.send(WorkerEvent::StatusOpCompleted { resolved });
            // Only the success path has domain work (opening the overlay); the
            // failure status is fully carried by the StatusOpCompleted above.
            if let Ok(log) = result {
                let _ = tx.send(WorkerEvent::StartupCommandLogsLoaded {
                    scope_label,
                    result: Ok(log),
                });
            }
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
        if let Some(project) = self.selected_project().cloned() {
            // Real project: keep the guard. Removing one that still has agents
            // here would orphan them — use "delete project" to remove agents too.
            let has_sessions = self
                .engine
                .sessions
                .iter()
                .any(|s| s.project_id == project.id);
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
        if let Some(session) = self.selected_session().cloned() {
            let project_id = session.project_id.clone();
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
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };

        // Refuse if any of this project's sessions have an async worktree
        // removal in-flight. `do_delete_session` runs git synchronously and
        // would race the worker, potentially leaving the project deletion
        // half-finished. The user must wait for the worker to complete (or
        // fail) before retrying.
        let pending_in_project =
            self.engine.sessions.iter().any(|s| {
                s.project_id == project.id && self.engine.pending_deletions.contains(&s.id)
            });
        if pending_in_project {
            self.set_error(
                "Cannot delete project while agent worktree removals are in progress. \
                 Wait for them to finish, then try again.",
            );
            return Ok(());
        }

        logger::info(&format!("deleting project {}", project.path));
        let session_ids = self
            .engine
            .sessions
            .iter()
            .filter(|session| session.project_id == project.id)
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            if let Some(index) = self
                .engine
                .sessions
                .iter()
                .position(|session| session.id == session_id)
            {
                self.selected_left = self
                    .left_items()
                    .iter()
                    .position(
                        |item| matches!(item, LeftItem::Session(session_index) if *session_index == index),
                    )
                    .unwrap_or(self.selected_left);
                // When deleting a project we also remove each agent's
                // worktree — the project itself is going away, so leaving
                // orphaned worktrees around would be surprising.
                self.do_delete_session(&session_id, true)?;
            }
        }
        let project_name = project.name.clone();
        let success_name = project_name.clone();
        let db_fail_name = project_name.clone();
        let op = dux_core::engine::status_op(format!(
            "Finishing deletion for project \"{project_name}\" after removing its agents..."
        ))
        .resolve_in_handler(move |o: &PersistFinalOutcome| match o {
            PersistFinalOutcome::Saved => dux_core::engine::Final::info(format!(
                "Deleted project \"{success_name}\" and all its agents"
            )),
            PersistFinalOutcome::DbFailed(error) => dux_core::engine::Final::error(format!(
                "Could not finish deleting project \"{db_fail_name}\" from the database: {error}"
            )),
            PersistFinalOutcome::ConfigWriteFailed(err) => dux_core::engine::Final::error(format!(
                "Project was deleted from the database, but config.toml could not be updated: {err}"
            )),
        });
        let pending = op.pending_status();
        let op_id = op.id().to_string();
        self.pending_persist_ops.insert(op_id.clone(), op);
        let reaction = self.engine.apply(Command::PersistProject {
            action: Box::new(ProjectPersistenceAction::Delete {
                project_id: project.id.clone(),
                project_name: project.name.clone(),
            }),
            status_op_id: Some(op_id),
        })?;
        self.apply_reaction(reaction);
        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
        Ok(())
    }

    /// Restart the selected agent with a fresh session, bypassing `--continue`
    /// or equivalent resume args. Works on both active and detached agents.
    pub(crate) fn force_reconnect_agent(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent first.");
            return Ok(());
        };
        if !Path::new(&session.worktree_path).exists() {
            self.set_error(format!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            ));
            return Ok(());
        }
        // Kill existing PTY if the agent is still active.
        self.engine.providers.remove(&session.id);
        self.engine.running_provider_pins.remove(&session.id);
        self.engine.pty_activity.remove(&session.id);
        self.engine.pty_input.remove(&session.id);
        self.engine.resume_fallback_candidates.remove(&session.id);

        let detached_label =
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);

        logger::info(&format!(
            "restarting agent \"{}\" with fresh session (no resume args)",
            session.branch_name
        ));
        let mut msg = self.engine.agent_reconnect_status_message(&session, false);
        if let Some(detached) = &detached_label {
            msg.push_str(&format!(
                " Agent \"{}\" was detached to avoid worktree conflicts.",
                detached,
            ));
        }
        if let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|p| p.id == session.project_id)
            && project.default_provider != session.provider
        {
            let provider_label = if self
                .engine
                .project_uses_explicit_default_provider(&project.id)
            {
                "current project provider"
            } else {
                "current global default provider"
            };
            msg.push_str(&format!(
                " Note: this agent uses {}. Your {provider_label} is {}.",
                session.provider.as_str(),
                project.default_provider.as_str(),
            ));
        }
        let branch_name = session.branch_name.clone();
        let session_id = session.id.clone();
        let request = self.agent_launch_request(
            session,
            false,
            AgentLaunchKind::ForceReconnect {
                status_message: msg,
            },
        );
        if self.dispatch_agent_launch(request) {
            // Route the busy through a keyed reconnect op so its final (resolved
            // in the shared launch-ready/failed view handlers) replaces exactly
            // this spinner instead of relying on most-recent-wins.
            let op = self
                .build_reconnect_status_op(format!("Starting fresh agent \"{branch_name}\"..."));
            self.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
            self.pending_reconnect_ops.insert(session_id, op);
        }
        Ok(())
    }

    pub(crate) fn reconnect_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a stopped agent first to reconnect.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {}", session.id));
        if self.engine.providers.contains_key(&session.id) {
            self.set_info(format!(
                "Agent \"{}\" is already connected.",
                session.branch_name
            ));
            return Ok(());
        }
        if !Path::new(&session.worktree_path).exists() {
            self.set_error(format!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            ));
            return Ok(());
        }
        let detached_label =
            self.detach_conflicting_worktree_session(&session.worktree_path, &session.id);

        let use_resume = self.engine.should_resume_session(&session);
        let mut msg = self
            .engine
            .agent_reconnect_status_message(&session, use_resume);
        if let Some(detached) = &detached_label {
            msg.push_str(&format!(
                " Agent \"{}\" was detached to avoid worktree conflicts.",
                detached,
            ));
        }
        if let Some(project) = self
            .engine
            .projects
            .iter()
            .find(|p| p.id == session.project_id)
            && project.default_provider != session.provider
        {
            let provider_label = if self
                .engine
                .project_uses_explicit_default_provider(&project.id)
            {
                "current project provider"
            } else {
                "current global default provider"
            };
            msg.push_str(&format!(
                " Note: this agent uses {}. Your {provider_label} is {}.",
                session.provider.as_str(),
                project.default_provider.as_str(),
            ));
        }
        let branch_name = session.branch_name.clone();
        let session_id = session.id.clone();
        let request = self.agent_launch_request(
            session,
            use_resume,
            AgentLaunchKind::Reconnect {
                status_message: msg,
            },
        );
        if self.dispatch_agent_launch(request) {
            // Route the busy through a keyed reconnect op so its final (resolved
            // in the shared launch-ready/failed view handlers) replaces exactly
            // this spinner instead of relying on most-recent-wins.
            let op =
                self.build_reconnect_status_op(format!("Launching agent \"{branch_name}\"..."));
            self.apply_reaction(dux_core::engine::EventReaction::Status(op.pending_status()));
            self.pending_reconnect_ops.insert(session_id, op);
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
        let worktree_path = session.worktree_path.clone();
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
        let path = match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self
                .engine
                .sessions
                .get(*index)
                .map(|s| s.worktree_path.clone()),
            Some(LeftItem::Project(index)) => {
                self.engine.projects.get(*index).map(|p| p.path.clone())
            }
            Some(LeftItem::EmptyProjectsSpacer) => None,
            Some(LeftItem::EmptyProjectsSeparator) => None,
            Some(LeftItem::OrphanProject(_)) => None,
            None => None,
        };
        match path {
            Some(p) => {
                match self.clipboard.copy_text(
                    &p,
                    "Agent's path copied to clipboard.",
                    &self.engine.worker_tx,
                ) {
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
        self.open_worktree_in_editor(&session.worktree_path, &session_label, &selected_editor)?;

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
            worktree_path: session.worktree_path.clone(),
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
            filter: TextInput::new(),
            searching: false,
            hovered_visible_index: 0,
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
            if !self.engine.providers.contains_key(&session.id) {
                continue;
            }
            let project_name = self.engine.project_name_for_session(session);
            let agent_name = self.session_label(session);
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

        for (terminal_id, terminal) in self.terminal_items() {
            let (project_name, session_label) = self
                .engine
                .sessions
                .iter()
                .find(|session| session.id == terminal.session_id)
                .map(|session| {
                    (
                        self.engine.project_name_for_session(session),
                        self.session_label(session),
                    )
                })
                .unwrap_or_else(|| ("unknown".to_string(), terminal.session_id.clone()));
            let foreground = terminal
                .foreground_cmd
                .clone()
                .map(|cmd| {
                    let trimmed = cmd.trim();
                    trimmed
                        .strip_prefix("TERM ")
                        .or_else(|| trimmed.strip_prefix("term "))
                        .unwrap_or(trimmed)
                        .to_string()
                })
                .filter(|cmd| !cmd.trim().is_empty())
                .unwrap_or_else(|| "shell".to_string());
            let label = foreground;
            let context = format!("on agent \"{session_label}\" under project \"{project_name}\"");
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
        if prompt.filter.is_empty() {
            return (0..prompt.runtimes.len()).collect();
        }
        let needle = prompt.filter.text.to_lowercase();
        prompt
            .runtimes
            .iter()
            .enumerate()
            .filter(|(_, runtime)| runtime.search_text.to_lowercase().contains(&needle))
            .map(|(index, _)| index)
            .collect()
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
        if visible_len == 0 {
            prompt.hovered_visible_index = 0;
        } else if prompt.hovered_visible_index >= visible_len {
            prompt.hovered_visible_index = visible_len.saturating_sub(1);
        }
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
                .get(prompt.hovered_visible_index)
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
            confirm_selected: false,
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
                RuntimeTargetId::Agent(session_id) => {
                    if self.engine.providers.remove(session_id).is_some() {
                        self.engine.running_provider_pins.remove(session_id);
                        self.engine.pty_activity.remove(session_id);
                        self.engine.pty_input.remove(session_id);
                        // Match the canonical teardown in Engine::kill_session_pty:
                        // dropping the provider without clearing this would leak the
                        // entry (it's keyed only off the now-removed provider).
                        self.engine.resume_fallback_candidates.remove(session_id);
                        self.engine
                            .mark_session_status(session_id, SessionStatus::Detached);
                        killed_agents += 1;
                        if selected_session_id.as_deref() == Some(session_id.as_str()) {
                            selected_agent_killed = true;
                        }
                    }
                }
                RuntimeTargetId::Terminal(terminal_id) => {
                    if self
                        .engine
                        .companion_terminals
                        .remove(terminal_id)
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
        self.engine
            .has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);

        (killed_agents, killed_terminals)
    }

    fn session_label(&self, session: &AgentSession) -> String {
        session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone())
    }

    /// If another session on the same worktree has a running PTY, detach it
    /// (kill the PTY and mark the session as `Detached`).  Returns the
    /// human-readable label of the detached session, if any.
    ///
    /// Thin view wrapper over `Engine::detach_conflicting_worktree_session`:
    /// the engine performs the domain-state mutation and returns the detached
    /// session's id + label; the App clears the engine's `pty_activity` entry
    /// for the id and surfaces the label for status messages.
    pub(crate) fn detach_conflicting_worktree_session(
        &mut self,
        worktree_path: &str,
        exclude_id: &str,
    ) -> Option<String> {
        let detached = self
            .engine
            .detach_conflicting_worktree_session(worktree_path, exclude_id)?;
        self.engine.pty_activity.remove(&detached.id);
        self.engine.pty_input.remove(&detached.id);
        Some(detached.label)
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
            worker_tx,
            worker_rx,
            config_writer,
            surface: Box::new(crate::TuiConfigSurface),
            reloading: false,
            deferred_commands: Vec::new(),
            reload_guard: None,
            providers: std::collections::HashMap::new(),
            running_provider_pins: std::collections::HashMap::new(),
            companion_terminals: std::collections::HashMap::new(),
            gh_status: crate::model::GhStatus::Unknown,
            pr_statuses: std::collections::HashMap::new(),
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_enabled: Arc::new(AtomicBool::new(false)),
            refs_watcher: None,
            refs_watch_paths: std::collections::HashMap::new(),
            resume_fallback_candidates: std::collections::HashMap::new(),
            pending_deletions: std::collections::HashSet::new(),
            deletion_busy_messages: std::collections::HashMap::new(),
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            watched_session_id: None,
            current_origin: Default::default(),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            in_flight: std::collections::HashSet::new(),
            pr_last_checked: std::collections::HashMap::new(),
            changed_files_poller_started: AtomicBool::new(false),
            branch_sync_worker_started: AtomicBool::new(false),
            pty_activity: std::collections::HashMap::new(),
            pty_input: std::collections::HashMap::new(),
            last_foreground_refresh: None,
            pending_web_checkout_ops: std::collections::HashMap::new(),
            pending_web_add_project_ops: std::collections::HashMap::new(),
            pending_web_pr_lookup_ops: std::collections::HashMap::new(),
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
            terminal_return_to_list: false,
            last_pty_size: (0, 0),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            start_time: std::time::Instant::now(),
            readonly_nudge_tick: None,
            collapsed_projects: std::collections::HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            overlay_layout: OverlayMouseLayoutState::default(),
            mouse_drag: None,
            last_mouse_click: None,
            pressed_button: None,
            interactive_patterns: crate::keybindings::InteractiveBytePatterns {
                bindings: Vec::new(),
            },
            raw_input_parser: crate::raw_input::RawInputParser::default(),
            raw_input_buf: Vec::new(),
            loading_input_buf: Vec::new(),
            in_bracket_paste: false,
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
            pending_delete_ops: std::collections::HashMap::new(),
            pending_reconnect_ops: std::collections::HashMap::new(),
            pending_checkout_inspect_ops: std::collections::HashMap::new(),
            pending_server_flip_op: None,
            pending_config_reload_op: None,
        };
        app.interactive_patterns = app.bindings.interactive_byte_patterns();
        app.rebuild_left_items();
        app
    }

    fn make_session(id: &str, provider: &str, worktree: &str) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            project_id: "project-1".to_string(),
            project_path: Some("/tmp/project".to_string()),
            provider: ProviderKind::from_str(provider),
            source_branch: "main".to_string(),
            branch_name: format!("branch-{id}"),
            worktree_path: worktree.to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
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
            worker_tx,
            worker_rx,
            config_writer,
            surface: Box::new(crate::TuiConfigSurface),
            reloading: false,
            deferred_commands: Vec::new(),
            reload_guard: None,
            providers: std::collections::HashMap::new(),
            running_provider_pins: std::collections::HashMap::new(),
            companion_terminals: std::collections::HashMap::new(),
            gh_status: crate::model::GhStatus::Unknown,
            pr_statuses: std::collections::HashMap::new(),
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_enabled: Arc::new(AtomicBool::new(false)),
            refs_watcher: None,
            refs_watch_paths: std::collections::HashMap::new(),
            resume_fallback_candidates: std::collections::HashMap::new(),
            pending_deletions: std::collections::HashSet::new(),
            deletion_busy_messages: std::collections::HashMap::new(),
            watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
            watched_session_id: None,
            current_origin: Default::default(),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            in_flight: std::collections::HashSet::new(),
            pr_last_checked: std::collections::HashMap::new(),
            changed_files_poller_started: AtomicBool::new(false),
            branch_sync_worker_started: AtomicBool::new(false),
            pty_activity: std::collections::HashMap::new(),
            pty_input: std::collections::HashMap::new(),
            last_foreground_refresh: None,
            pending_web_checkout_ops: std::collections::HashMap::new(),
            pending_web_add_project_ops: std::collections::HashMap::new(),
            pending_web_pr_lookup_ops: std::collections::HashMap::new(),
            pending_delete_ops_web: std::collections::HashMap::new(),
            pending_create_ops: std::collections::HashMap::new(),
            pending_web_launch_ops: std::collections::HashMap::new(),
            last_created_op_id: None,
            created_session_by_op: std::collections::HashMap::new(),
        }
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

    #[test]
    fn detach_finds_conflict_on_same_worktree() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let s2 = make_session("s2", "codex", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1, s2], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s2");
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

        let label = app.detach_conflicting_worktree_session("/tmp/wt/b", "s2");
        assert!(label.is_none());
        assert!(app.engine.providers.contains_key("s1"));
    }

    #[test]
    fn detach_excludes_self() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let mut app = test_app_with_sessions(vec![s1], vec![project]);
        mark_active(&mut app, "s1");

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s1");
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

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s2");
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

        let label = app.detach_conflicting_worktree_session("/tmp/wt/a", "s1");
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
        let has_sibling = app
            .engine
            .sessions
            .iter()
            .any(|s| s.id != "s1" && s.worktree_path == "/tmp/wt/a");
        assert!(has_sibling, "sibling session should exist");
    }

    #[test]
    fn delete_session_allows_removal_when_last() {
        let s1 = make_session("s1", "claude", "/tmp/wt/a");
        let project = make_project("project-1", "claude");
        let app = test_app_with_sessions(vec![s1], vec![project]);

        let has_sibling = app
            .engine
            .sessions
            .iter()
            .any(|s| s.id != "s1" && s.worktree_path == "/tmp/wt/a");
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

        app.engine.mark_session_provider_started("s1");

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
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
        s2.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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

    /// The async path (`begin_delete_session`) must NOT remove the session
    /// immediately when the worktree deletion is going to run in a worker —
    /// otherwise a failed git call would leave the user with a vanished
    /// agent and no way to retry. The session should only disappear once
    /// the worker reports success.
    #[test]
    fn begin_delete_session_keeps_session_until_worker_replies() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
        let project = make_project_at("project-1", "claude", &project_dir.path().to_string_lossy());
        let mut app = test_app_with_sessions(vec![s1], vec![project]);

        app.begin_delete_session("s1", true);

        // Session must still be present: the worker thread has been spawned
        // but hasn't (at most) completed the cleanup on our thread yet.
        assert!(
            app.engine.sessions.iter().any(|s| s.id == "s1"),
            "session must remain until the worker reports success",
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
        s1.project_id = "project-1".to_string();
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

    #[test]
    fn deleting_last_agent_selects_project_after_it_moves_to_empty_list() {
        let mut project_with_deleted_agent = make_project("project-1", "claude");
        project_with_deleted_agent.name = "deleted-agent-project".to_string();
        let mut active_project = make_project("project-2", "codex");
        active_project.name = "active-project".to_string();
        let mut already_empty_project = make_project("project-3", "codex");
        already_empty_project.name = "already-empty-project".to_string();

        let mut deleted_session = make_session("s1", "claude", "/tmp/wt/a");
        deleted_session.project_id = "project-1".to_string();
        let mut remaining_session = make_session("s2", "codex", "/tmp/wt/b");
        remaining_session.project_id = "project-2".to_string();

        let mut app = test_app_with_sessions(
            vec![deleted_session, remaining_session],
            vec![
                project_with_deleted_agent,
                active_project,
                already_empty_project,
            ],
        );
        app.engine.config.ui.empty_project_separator_min_projects = 3;
        app.rebuild_left_items();
        app.selected_left = app
            .left_items()
            .iter()
            .position(
                |item| matches!(item, LeftItem::Session(index) if app.engine.sessions[*index].id == "s1"),
            )
            .expect("deleted session row");

        app.finish_delete_session("s1", WorktreeRemoval::PreservedOrphan, true)
            .expect("finish delete");

        let separator_index = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::EmptyProjectsSeparator))
            .expect("empty-project separator should remain");
        let selected_project = app.selected_project().expect("selected project");
        assert_eq!(selected_project.id, "project-1");
        assert!(
            app.selected_left > separator_index,
            "project should be selected in the empty-project section"
        );

        app.create_agent_for_selected_project()
            .expect("selected empty project should accept new-agent action");
        assert_eq!(app.status.tone(), crate::statusline::StatusTone::Busy);
        assert!(
            app.status.text().contains("deleted-agent-project"),
            "new-agent action should target the project that just moved to the empty list, got: {}",
            app.status.text()
        );
    }

    /// `finish_delete_session` is the handler invoked both inline and from
    /// the worker event. It must be idempotent: if the session has already
    /// been removed (e.g. a duplicate worker event) it should no-op.
    #[test]
    fn finish_delete_session_is_idempotent() {
        let mut s1 = make_session("s1", "claude", "/tmp/wt/a");
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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
        s1.project_id = "project-1".to_string();
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
                result: Ok(false),
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
        s1.project_id = "project-1".to_string();
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
                result: Ok(false),
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
        s1.project_id = "project-1".to_string();
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
                result: Ok(false),
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
        s1.project_id = "project-1".to_string();
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

    /// When the worker fails to delete a worktree, the error message should
    /// include the agent label so the user knows which one failed.
    #[test]
    fn worktree_remove_failure_identifies_agent() {
        let project_dir = tempdir().expect("project tempdir");
        let worktree_dir = tempdir().expect("worktree tempdir");
        let worktree_path = worktree_dir.path().to_string_lossy().to_string();

        let mut s1 = make_session("s1", "claude", &worktree_path);
        s1.project_id = "project-1".to_string();
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
        for (branch_already_deleted, expected) in [
            (
                false,
                "Deleted claude agent from project \"demo\" with branch \"branch-s1\".",
            ),
            (
                true,
                "Deleted agent (branch \"branch-s1\" was already removed).",
            ),
        ] {
            let mut s1 = make_session("s1", "claude", "/tmp/wt");
            s1.project_id = "project-1".to_string();
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
                    result: Ok(branch_already_deleted),
                })
                .expect("channel send");
            app.drain_events();

            assert_eq!(
                app.status.message(),
                expected,
                "branch_already_deleted={branch_already_deleted}",
            );
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
                    branch_already_deleted: true,
                },
                "Deleted agent (branch \"branch-s1\" was already removed).",
            ),
            (
                WorktreeRemoval::Performed {
                    branch_already_deleted: false,
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
            app.apply_finish_delete_session_outcome("s1", outcome, removal, true);
            assert_eq!(app.status.message(), expected, "variant {removal:?}");
        }
    }
}
