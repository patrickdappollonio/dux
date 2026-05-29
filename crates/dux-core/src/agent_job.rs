//! Standalone background-job functions that spawn a CLI provider PTY for a
//! new or relaunching agent. Called from the App's
//! `dispatch_create_agent_request` and `dispatch_agent_launch` worker
//! threads; both functions post results back via `worker_tx`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use chrono::Utc;
use uuid::Uuid;

use crate::config::{Config, DuxPaths, check_provider_available, provider_config};
use crate::model::{AgentSession, SessionStatus};
use crate::startup::{StartupCommandRun, run_startup_command};
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchKind, AgentLaunchReadyData, AgentLaunchRequest,
    CreateAgentRequest, WorkerEvent,
};
use crate::{gh, git, logger};

pub fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
) {
    let (
        project,
        provider,
        source_branch,
        status_message,
        branch_name,
        worktree_path,
        owns_worktree,
        title,
        launch_with_resume,
    ) = match request {
        CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
            pull_before_create,
        } => {
            let repo_path = PathBuf::from(&project.path);

            if pull_before_create {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Pulling latest changes for project \"{}\" before creating the agent...",
                    project.name
                )));
                let leading_branch = project
                    .leading_branch
                    .clone()
                    .unwrap_or_else(|| project.current_branch.clone());
                if let Err(err) = git::switch_branch_if_needed(&repo_path, &leading_branch)
                    .and_then(|_| {
                        if git::has_tracked_changes(&repo_path)? {
                            return Err(anyhow::anyhow!("source checkout has uncommitted changes"));
                        }
                        git::pull_branch(&repo_path, &leading_branch)
                    })
                {
                    logger::error(&format!(
                        "pre-create pull failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to pull latest changes for project \"{}\" before creating the agent: {err}",
                        project.name
                    )));
                    return;
                }
            }

            // Resolve the branch name early so we can check for an
            // existing branch before calling git worktree add.  When no
            // custom name was provided, a random pet name is generated.
            let resolved_name = custom_name.unwrap_or_else(git::docker_style_name);

            // If the caller already confirmed via the UI dialog,
            // `use_existing_branch` is true.  Otherwise, do a last-mile
            // check — this covers auto-generated pet names that
            // coincidentally match an existing branch.
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();
            let leading_branch = project
                .leading_branch
                .clone()
                .unwrap_or_else(|| project.current_branch.clone());
            if !attach_existing && !git::local_branch_exists(&repo_path, &leading_branch) {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Cannot create agent for \"{}\": leading branch \"{}\" no longer exists locally. Restore that branch or re-add the project.",
                    project.name, leading_branch
                )));
                return;
            }

            let progress = if attach_existing {
                format!(
                    "Attaching to existing branch \"{}\" for project \"{}\"...",
                    resolved_name, project.name
                )
            } else {
                format!(
                    "Creating a new worktree for project \"{}\"...",
                    project.name
                )
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(progress));

            let (branch_name, worktree_path) = if attach_existing {
                match git::create_worktree_existing_branch(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    &resolved_name,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "worktree creation (existing branch) failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                            "Failed to attach to existing branch for project \"{}\": {err}",
                            project.name
                        )));
                        return;
                    }
                }
            } else {
                match git::create_worktree_from_start_point(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    Some(&leading_branch),
                    Some(&resolved_name),
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "worktree creation failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                            "Failed to create a new worktree for project \"{}\": {err}",
                            project.name
                        )));
                        return;
                    }
                }
            };
            let status_message = if attach_existing {
                format!(
                    "Attached to existing branch \"{}\" in project \"{}\". The worktree is ready in a fresh session.",
                    branch_name, project.name
                )
            } else {
                format!(
                    "Created {} agent \"{}\" in project \"{}\". The new worktree is ready in a fresh session.",
                    project.default_provider.as_str(),
                    branch_name,
                    project.name
                )
            };
            (
                project.clone(),
                project.default_provider.clone(),
                if attach_existing {
                    project.current_branch.clone()
                } else {
                    leading_branch
                },
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
        CreateAgentRequest::PullRequest {
            project,
            host,
            owner_repo,
            number,
            title,
            state,
            head_branch,
            custom_name,
            use_existing_branch,
        } => {
            let repo_path = PathBuf::from(&project.path);
            let resolved_name = custom_name.unwrap_or_else(|| head_branch.clone());
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();

            if attach_existing {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Attaching to existing branch \"{}\" for PR #{} in project \"{}\"...",
                    resolved_name, number, project.name
                )));
            } else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                    "Fetching PR #{} from {} into branch \"{}\"...",
                    number, owner_repo, resolved_name
                )));
                if let Err(err) = git::fetch_pull_request_head(&repo_path, number, &resolved_name) {
                    logger::error(&format!(
                        "PR worktree fetch failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to fetch PR #{} from {}: {err}",
                        number, owner_repo
                    )));
                    return;
                }
            }

            let (branch_name, worktree_path) = match git::create_worktree_existing_branch(
                &repo_path,
                &paths.worktrees_root,
                &project.name,
                &resolved_name,
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "PR worktree creation failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a worktree for PR #{} in project \"{}\": {err}",
                        number, project.name
                    )));
                    return;
                }
            };
            let status_message = format!(
                "Created {} agent \"{}\" from PR #{} ({}) in project \"{}\".",
                project.default_provider.as_str(),
                branch_name,
                number,
                title,
                project.name
            );
            logger::info(&format!(
                "created PR worktree from {} #{} ({state}) {}",
                owner_repo,
                number,
                gh::pull_request_url(&host, &owner_repo, number)
            ));
            (
                project.clone(),
                project.default_provider.clone(),
                project.current_branch.clone(),
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
        CreateAgentRequest::ForkSession {
            project,
            source_session,
            source_label,
            custom_name,
        } => {
            let Some(custom_name) = custom_name else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(
                    "Forking an agent requires choosing a name first.".to_string(),
                ));
                return;
            };
            let source_worktree = PathBuf::from(&source_session.worktree_path);
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Creating a forked worktree from agent \"{source_label}\"...",
            )));
            let source_head = match git::head_commit(&source_worktree) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_session.worktree_path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect the source worktree for agent \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let repo_path = PathBuf::from(&project.path);
            let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                &repo_path,
                &paths.worktrees_root,
                &project.name,
                Some(&source_head),
                Some(&custom_name),
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "fork worktree creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a forked worktree from agent \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Copying the current filesystem contents from agent \"{source_label}\" into the new fork...",
            )));
            if let Err(err) = git::mirror_worktree_contents(&source_worktree, &worktree_path) {
                logger::error(&format!(
                    "failed to mirror worktree {} into {}: {err}",
                    source_worktree.display(),
                    worktree_path.display()
                ));
                let _ = git::remove_worktree(&repo_path, &worktree_path, &branch_name);
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Failed to copy the source worktree contents for agent \"{source_label}\": {err}",
                )));
                return;
            }
            let status_message = format!(
                "Forked {} agent \"{}\" from \"{}\" in project \"{}\". The new worktree starts with copied files and a fresh session.",
                source_session.provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            (
                project,
                source_session.provider,
                source_session.branch_name,
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
        CreateAgentRequest::ExistingManagedWorktree {
            project,
            worktree_path,
            branch_name,
            custom_name,
        } => {
            let agent_name = custom_name.clone().unwrap_or_else(|| branch_name.clone());
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Launching {} in existing worktree \"{}\"...",
                project.default_provider.as_str(),
                worktree_path.display(),
            )));
            let status_message = format!(
                "Imported {} agent \"{}\" from existing managed worktree for project \"{}\".",
                project.default_provider.as_str(),
                agent_name,
                project.name
            );
            (
                project.clone(),
                project.default_provider.clone(),
                branch_name.clone(),
                status_message,
                branch_name,
                worktree_path,
                false,
                custom_name,
                true,
            )
        }
        CreateAgentRequest::ForkExternalWorktree {
            project,
            source_worktree_path,
            source_label,
            source_branch,
            custom_name,
        } => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Creating a managed worktree from external worktree \"{source_label}\"...",
            )));
            let source_head = match git::head_commit(&source_worktree_path) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_worktree_path.display()
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect external worktree \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let repo_path = PathBuf::from(&project.path);
            let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                &repo_path,
                &paths.worktrees_root,
                &project.name,
                Some(&source_head),
                custom_name.as_deref(),
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "external worktree fork creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a managed worktree from external worktree \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Copying dirty and untracked files from external worktree \"{source_label}\"...",
            )));
            if let Err(err) = git::mirror_worktree_contents(&source_worktree_path, &worktree_path) {
                logger::error(&format!(
                    "failed to mirror external worktree {} into {}: {err}",
                    source_worktree_path.display(),
                    worktree_path.display()
                ));
                let _ = git::remove_worktree(&repo_path, &worktree_path, &branch_name);
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Failed to copy external worktree contents from \"{source_label}\": {err}",
                )));
                return;
            }
            let status_message = format!(
                "Created {} agent \"{}\" from external worktree \"{}\" in project \"{}\". Dirty and untracked files were copied into the managed worktree.",
                project.default_provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            (
                project.clone(),
                project.default_provider.clone(),
                source_branch,
                status_message,
                branch_name,
                worktree_path,
                true,
                None,
                false,
            )
        }
    };
    let repo_path = PathBuf::from(&project.path);
    if owns_worktree {
        logger::info(&format!(
            "created worktree {} on branch {}",
            worktree_path.display(),
            branch_name
        ));
    } else {
        logger::info(&format!(
            "reusing worktree {} on branch {} for new provider session",
            worktree_path.display(),
            branch_name
        ));
    }
    let started_providers = if launch_with_resume {
        vec![provider.as_str().to_string()]
    } else {
        Vec::new()
    };
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider,
        source_branch,
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title,
        started_providers,
        desired_running: true,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        if owns_worktree {
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
        }
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(hint));
        return;
    }
    let env = match crate::config::resolve_agent_env(&config.env, &project.env) {
        Ok(env) => env,
        Err(err) => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Invalid environment variables for project \"{}\": {err:#}",
                project.name
            )));
            return;
        }
    };
    let startup_result = project
        .startup_command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Running startup command for agent \"{}\"...",
                session.branch_name
            )));
            run_startup_command(
                &paths,
                StartupCommandRun {
                    project: project.clone(),
                    session: session.clone(),
                    command: command.to_string(),
                    terminal: config.startup_command_terminal.clone(),
                    env: env.clone(),
                },
            )
        });
    if let Some(result) = &startup_result {
        match &result.status {
            Ok(()) => logger::info(&format!(
                "startup command succeeded for {} (log: {})",
                result.session_id,
                result.log_path.display()
            )),
            Err(err) => logger::error(&format!(
                "startup command failed for {}: {err} (log: {})",
                result.session_id,
                result.log_path.display()
            )),
        }
    }
    let launch_message = if launch_with_resume {
        format!(
            "Continuing {} in the existing worktree...",
            session.provider.as_str()
        )
    } else {
        format!(
            "Launching {} in a fresh session...",
            session.provider.as_str()
        )
    };
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(launch_message));
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let request = AgentLaunchRequest {
        session,
        provider_config: provider_cfg,
        env,
        resume: launch_with_resume,
        pty_size: (rows, cols),
        scrollback_lines: config.ui.agent_scrollback_lines,
        kind: AgentLaunchKind::Create {
            status_message,
            repo_path: repo_path.to_string_lossy().to_string(),
            owns_worktree,
            startup_result,
        },
    };
    run_agent_launch_job(request, worker_tx);
}

pub fn run_agent_launch_job(request: AgentLaunchRequest, worker_tx: Sender<WorkerEvent>) {
    let launch_args = request.provider_config.interactive_args(request.resume);
    let (rows, cols) = request.pty_size;
    logger::debug(&format!(
        "spawning PTY {:?} {:?} in {} ({}x{}, resume_supported={})",
        request.provider_config.command,
        launch_args,
        request.session.worktree_path,
        cols,
        rows,
        request.provider_config.supports_session_resume()
    ));

    if let Err(message) = check_provider_available(&request.provider_config) {
        logger::error(&format!(
            "provider availability check failed for {}: {message}",
            request.session.id
        ));
        if let AgentLaunchKind::Create {
            repo_path,
            owns_worktree,
            ..
        } = &request.kind
            && *owns_worktree
        {
            let _ = git::remove_worktree(
                Path::new(repo_path),
                Path::new(&request.session.worktree_path),
                &request.session.branch_name,
            );
        }
        let _ = worker_tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
            AgentLaunchFailedData { request, message },
        )));
        return;
    }

    let client = match crate::pty::PtyClient::spawn_with_env(
        &request.provider_config.command,
        &launch_args,
        Path::new(&request.session.worktree_path),
        rows,
        cols,
        request.scrollback_lines,
        &request.env,
    ) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!(
                "PTY spawn failed for {}: {err}",
                request.session.id
            ));
            if let AgentLaunchKind::Create {
                repo_path,
                owns_worktree,
                ..
            } = &request.kind
                && *owns_worktree
            {
                let _ = git::remove_worktree(
                    Path::new(repo_path),
                    Path::new(&request.session.worktree_path),
                    &request.session.branch_name,
                );
            }
            let message = if matches!(request.kind, AgentLaunchKind::Create { .. }) {
                format!("Failed to start {}: {err}", request.provider_config.command)
            } else {
                err.to_string()
            };
            let _ = worker_tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
                AgentLaunchFailedData { request, message },
            )));
            return;
        }
    };
    logger::info(&format!("PTY session started for {}", request.session.id));
    let _ = worker_tx.send(WorkerEvent::AgentLaunchReady(Box::new(
        AgentLaunchReadyData { request, client },
    )));
}
