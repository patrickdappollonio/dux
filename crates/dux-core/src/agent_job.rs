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

/// What to do when the copy's HEAD-equality guard fails (the source checkout
/// and the new worktree are on different commits, so the status delta would
/// not describe the same base tree).
enum HeadMismatch {
    /// Skip the copy and append a visible note to the create status (fresh
    /// agents: the project checkout simply is not on that branch's commit).
    SkipWithNote { branch: String },
    /// Fail the creation (forks: equal HEADs hold by construction, so a
    /// mismatch means the source moved mid-create).
    Fail,
}

/// A copy of uncommitted changes planned by a per-request arm and executed in
/// the common tail, after the provider availability check (so a missing
/// provider does not throw away completed copy work with the worktree).
struct PendingCopy {
    /// The project checkout, or the fork's source worktree.
    source: PathBuf,
    /// Short human description of the source for progress/error messages.
    source_desc: String,
    on_head_mismatch: HeadMismatch,
}

#[allow(clippy::too_many_arguments)]
pub fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
    status_op_id: String,
    identity: crate::term_identity::TerminalIdentity,
) {
    // The opaque id of the shared create-agent `HandlerStatusOp` keys every
    // progress/failure event and is carried in `AgentLaunchKind::Create` so the
    // launch-ready/failed handler can resolve the op's final on the same id.
    let create_key = status_op_id;
    // Non-fatal notes (best-effort pull problems, skipped copies) accumulated
    // across the job and appended to the create status message, so they ride
    // the keyed create-op final and stay visible.
    let mut creation_notes: Vec<String> = Vec::new();
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
        pending_copy,
    ) = match request {
        CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
            pull_before_create,
            copy_uncommitted_changes,
        } => {
            let repo_path = PathBuf::from(&project.path);
            let leading_branch = project.leading_branch.clone().unwrap_or_else(|| {
                let cur =
                    (!project.current_branch.is_empty()).then_some(project.current_branch.as_str());
                crate::project_browser::leading_branch_for_project(&repo_path, cur)
            });

            if pull_before_create {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                    status_op_id: create_key.clone(),
                    message: format!(
                        "Pulling latest changes for project \"{}\" before creating the agent...",
                        project.name
                    ),
                });
                // The pull is best-effort: creation never aborts on a failed
                // switch or pull. The agent simply starts from the local
                // branch state, and the create status says so.
                if let Err(err) = git::switch_branch_if_needed(&repo_path, &leading_branch) {
                    logger::error(&format!(
                        "pre-create branch switch failed for {}: {err}",
                        project.path
                    ));
                    creation_notes.push(format!(
                        "Warning: could not switch the project checkout to \"{leading_branch}\": {err}. The agent starts from the local branch state."
                    ));
                    // Deliberately skip the pull after a failed switch:
                    // `pull_branch` re-runs the same switch internally, so it
                    // would just re-fail and duplicate the warning.
                } else {
                    match git::has_origin_remote(&repo_path) {
                        Ok(true) => {
                            if let Err(err) = git::pull_branch(&repo_path, &leading_branch) {
                                logger::error(&format!(
                                    "pre-create pull failed for {}: {err}",
                                    project.path
                                ));
                                creation_notes.push(format!(
                                    "Warning: could not pull \"{leading_branch}\" from origin: {err}. The agent starts from the local branch state."
                                ));
                            }
                        }
                        Ok(false) => {
                            // Steady state for local-only repos: the pull was
                            // never promised, so this is a log-only skip.
                            logger::info(&format!(
                                "skipping pre-create pull for {}: no origin remote",
                                project.path
                            ));
                        }
                        Err(err) => {
                            logger::error(&format!(
                                "pre-create origin check failed for {}: {err}",
                                project.path
                            ));
                            creation_notes.push(format!(
                                "Warning: could not check for an origin remote: {err}. The agent starts from the local branch state."
                            ));
                        }
                    }
                }
            }

            // A user-typed name becomes the agent's durable `title` (identity),
            // not just the branch name. An auto-generated pet name (custom_name
            // is None) leaves `title` empty so the display keeps tracking the
            // branch — there is nothing user-authored to protect.
            let title = custom_name.clone();
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
            // Distinguish an unborn repo (no commits at all) from a genuinely
            // deleted branch. `local_branch_exists` is false for both, but the
            // remedies differ: an unborn repo needs an initial commit, not a
            // "restore the branch" that never existed. Check commits first so
            // the message is accurate for a freshly `git init`'d project that
            // slipped in (e.g. added via the raw API, or hand-written config).
            // Only a CONFIRMED unborn HEAD takes this branch — an indeterminate
            // git result falls through to the branch-missing check rather than
            // wrongly advising "create an initial commit" on a repo with history.
            if !attach_existing && git::repo_commit_state(&repo_path) == git::CommitState::Unborn {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed { status_op_id: create_key.clone(), message: format!(
                    "Cannot create agent for \"{}\": the repository at {} has no commits yet. Create an initial commit (for example `git commit --allow-empty -m \"Initial commit\"`), then try again.",
                    project.name, repo_path.display()
                ) });
                return;
            }
            if !attach_existing && !git::local_branch_exists(&repo_path, &leading_branch) {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed { status_op_id: create_key.clone(), message: format!(
                    "Cannot create agent for \"{}\": leading branch \"{}\" no longer exists locally. Restore that branch or re-add the project.",
                    project.name, leading_branch
                ) });
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
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: progress,
            });

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
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                            status_op_id: create_key.clone(),
                            message: format!(
                                "Failed to attach to existing branch for project \"{}\": {err}",
                                project.name
                            ),
                        });
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
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                            status_op_id: create_key.clone(),
                            message: format!(
                                "Failed to create a new worktree for project \"{}\": {err}",
                                project.name
                            ),
                        });
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
            // The copy runs in the common tail (after the provider check).
            // No per-path exceptions: the HEAD-equality guard decides, for
            // fresh worktrees and attached existing branches alike.
            let pending_copy = copy_uncommitted_changes.then(|| PendingCopy {
                source: repo_path.clone(),
                source_desc: format!("project \"{}\"", project.name),
                on_head_mismatch: HeadMismatch::SkipWithNote {
                    branch: if attach_existing {
                        resolved_name.clone()
                    } else {
                        leading_branch.clone()
                    },
                },
            });
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
                title,
                false,
                pending_copy,
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
            // A typed name is the agent's durable title; falling back to the PR
            // head branch means no user-authored name, so leave title empty.
            // (Named `agent_title` to avoid shadowing the PR `title` above.)
            let agent_title = custom_name.clone();
            let resolved_name = custom_name.unwrap_or_else(|| head_branch.clone());
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();

            if attach_existing {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                    status_op_id: create_key.clone(),
                    message: format!(
                        "Attaching to existing branch \"{}\" for PR #{} in project \"{}\"...",
                        resolved_name, number, project.name
                    ),
                });
            } else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                    status_op_id: create_key.clone(),
                    message: format!(
                        "Fetching PR #{} from {} into branch \"{}\"...",
                        number, owner_repo, resolved_name
                    ),
                });
                if let Err(err) = git::fetch_pull_request_head(&repo_path, number, &resolved_name) {
                    logger::error(&format!(
                        "PR worktree fetch failed for {} #{}: {err}",
                        owner_repo, number
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: create_key.clone(),
                        message: format!(
                            "Failed to fetch PR #{} from {}: {err}",
                            number, owner_repo
                        ),
                    });
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
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: create_key.clone(),
                        message: format!(
                            "Failed to create a worktree for PR #{} in project \"{}\": {err}",
                            number, project.name
                        ),
                    });
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
                agent_title,
                false,
                None,
            )
        }
        CreateAgentRequest::ForkSession {
            project,
            source_session,
            source_label,
            custom_name,
        } => {
            let Some(custom_name) = custom_name else {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                    status_op_id: create_key.clone(),
                    message: "Forking an agent requires choosing a name first.".to_string(),
                });
                return;
            };
            let source_worktree = PathBuf::from(&source_session.worktree_path);
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: format!("Creating a forked worktree from agent \"{source_label}\"..."),
            });
            let source_head = match git::head_commit(&source_worktree) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_session.worktree_path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed { status_op_id: create_key.clone(), message: format!(
                        "Failed to inspect the source worktree for agent \"{source_label}\": {err}",
                    ) });
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
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed { status_op_id: create_key.clone(), message: format!(
                        "Failed to create a forked worktree from agent \"{source_label}\": {err}",
                    ) });
                    return;
                }
            };
            let status_message = format!(
                "Forked {} agent \"{}\" from \"{}\" in project \"{}\". The new worktree starts with the copied uncommitted and untracked changes (gitignored files are not copied) and a fresh session.",
                source_session.provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            // Equal HEADs hold by construction (the worktree was just created
            // from `source_head`); the copy itself runs in the common tail.
            let pending_copy = Some(PendingCopy {
                source: source_worktree,
                source_desc: format!("agent \"{source_label}\""),
                on_head_mismatch: HeadMismatch::Fail,
            });
            (
                project,
                source_session.provider,
                source_session.branch_name,
                status_message,
                branch_name,
                worktree_path,
                true,
                // A fork always requires a chosen name; persist it as the
                // agent's durable title.
                Some(custom_name),
                false,
                pending_copy,
            )
        }
        CreateAgentRequest::ExistingManagedWorktree {
            project,
            worktree_path,
            branch_name,
            custom_name,
        } => {
            let agent_name = custom_name.clone().unwrap_or_else(|| branch_name.clone());
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: format!(
                    "Launching {} in existing worktree \"{}\"...",
                    project.default_provider.as_str(),
                    worktree_path.display(),
                ),
            });
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
                None,
            )
        }
        CreateAgentRequest::ForkExternalWorktree {
            project,
            source_worktree_path,
            source_label,
            source_branch,
            custom_name,
        } => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: format!(
                    "Creating a managed worktree from external worktree \"{source_label}\"...",
                ),
            });
            let source_head = match git::head_commit(&source_worktree_path) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_worktree_path.display()
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: create_key.clone(),
                        message: format!(
                            "Failed to inspect external worktree \"{source_label}\": {err}",
                        ),
                    });
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
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed { status_op_id: create_key.clone(), message: format!(
                        "Failed to create a managed worktree from external worktree \"{source_label}\": {err}",
                    ) });
                    return;
                }
            };
            let status_message = format!(
                "Created {} agent \"{}\" from external worktree \"{}\" in project \"{}\". Uncommitted and untracked changes were copied into the managed worktree (gitignored files are not copied).",
                project.default_provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            // Equal HEADs hold by construction (the worktree was just created
            // from the external worktree's head); the copy runs in the tail.
            let pending_copy = Some(PendingCopy {
                source: source_worktree_path,
                source_desc: format!("external worktree \"{source_label}\""),
                on_head_mismatch: HeadMismatch::Fail,
            });
            (
                project.clone(),
                project.default_provider.clone(),
                source_branch,
                status_message,
                branch_name,
                worktree_path,
                true,
                // A typed name becomes the agent's durable title; None leaves the
                // display tracking the branch (an auto-derived worktree name).
                custom_name,
                false,
                pending_copy,
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
        // The agent is born on `branch_name`; record that as its immutable
        // original branch. The branch-sync poller and intentional renames update
        // `branch_name` later but must never touch `initial_branch`.
        initial_branch: branch_name.clone(),
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title,
        started_providers,
        desired_running: true,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_focused_tab: None,
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
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
            status_op_id: create_key.clone(),
            message: hint,
        });
        return;
    }
    // The planned copy of uncommitted changes runs here, after the provider
    // availability check (so a missing provider does not discard completed
    // copy work) and before the startup command (which must see the files).
    if let Some(copy) = pending_copy {
        let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
            status_op_id: create_key.clone(),
            message: format!(
                "Copying uncommitted and untracked changes from {} into the new worktree (gitignored files are not copied)...",
                copy.source_desc
            ),
        });
        let worktree = Path::new(&session.worktree_path);
        // Porcelain status is relative to the HEAD commit's tree, so the copy
        // is only faithful when both sides sit on the same commit. A failed
        // `head_commit` is NOT a branch mismatch: it is reported as its own
        // verification failure so the user is not told the wrong cause.
        let head_check = match (git::head_commit(&copy.source), git::head_commit(worktree)) {
            (Ok(source_head), Ok(worktree_head)) if source_head == worktree_head => Ok(true),
            (Ok(_), Ok(_)) => Ok(false),
            (Err(err), _) | (_, Err(err)) => Err(err),
        };
        match head_check {
            Ok(true) => match git::copy_uncommitted_changes(&copy.source, worktree) {
                Ok(summary) => {
                    if !summary.skipped_paths.is_empty() {
                        creation_notes.push(format!(
                                "Some paths were not copied (submodules, embedded repositories, or special files): {}.",
                                summary.skipped_paths.join(", ")
                            ));
                    }
                }
                Err(err) => {
                    logger::error(&format!(
                        "failed to copy uncommitted changes from {} into {}: {err}",
                        copy.source.display(),
                        session.worktree_path
                    ));
                    if owns_worktree {
                        let _ = git::remove_worktree(
                            &repo_path,
                            Path::new(&session.worktree_path),
                            &session.branch_name,
                        );
                    }
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                        status_op_id: create_key.clone(),
                        message: format!(
                            "Failed to copy uncommitted changes from {}: {err}",
                            copy.source_desc
                        ),
                    });
                    return;
                }
            },
            not_equal_or_failed => {
                let check_error = match not_equal_or_failed {
                    Err(err) => {
                        logger::error(&format!(
                            "uncommitted-changes copy: could not resolve HEAD for {} or {}: {err}",
                            copy.source.display(),
                            session.worktree_path
                        ));
                        Some(err)
                    }
                    _ => None,
                };
                match copy.on_head_mismatch {
                    HeadMismatch::SkipWithNote { branch } => {
                        creation_notes.push(match &check_error {
                            Some(err) => format!(
                                "Uncommitted changes were not copied: could not verify the checkout's commit: {err}."
                            ),
                            None => format!(
                                "Uncommitted changes were not copied: the project checkout is not on \"{branch}\"'s commit."
                            ),
                        });
                    }
                    HeadMismatch::Fail => {
                        if check_error.is_none() {
                            logger::error(&format!(
                                "uncommitted-changes copy aborted: {} is no longer on the commit {} was created from",
                                copy.source.display(),
                                session.worktree_path
                            ));
                        }
                        if owns_worktree {
                            let _ = git::remove_worktree(
                                &repo_path,
                                Path::new(&session.worktree_path),
                                &session.branch_name,
                            );
                        }
                        let message = match &check_error {
                            Some(err) => format!(
                                "Failed to copy uncommitted changes from {}: could not verify the source worktree's commit: {err}.",
                                copy.source_desc
                            ),
                            None => format!(
                                "Failed to copy uncommitted changes from {}: the source moved to a different commit during creation.",
                                copy.source_desc
                            ),
                        };
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                            status_op_id: create_key.clone(),
                            message,
                        });
                        return;
                    }
                }
            }
        }
    }
    // Notes ride the keyed create-op final so they surface as the visible
    // status/toast, never log-only.
    let status_message = if creation_notes.is_empty() {
        status_message
    } else {
        format!("{status_message} {}", creation_notes.join(" "))
    };
    let env = match crate::config::resolve_agent_env(&config.env, &project.env) {
        Ok(env) => env,
        Err(err) => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed {
                status_op_id: create_key.clone(),
                message: format!(
                    "Invalid environment variables for project \"{}\": {err:#}",
                    project.name
                ),
            });
            return;
        }
    };
    let startup_result = project
        .startup_command
        .as_deref()
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(|command| {
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
                status_op_id: create_key.clone(),
                message: format!(
                    "Running startup command for agent \"{}\"...",
                    session.branch_name
                ),
            });
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
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress {
        status_op_id: create_key.clone(),
        message: launch_message,
    });
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let request = AgentLaunchRequest {
        // Create is always the session-slot tab: tab_id == session.id, effective
        // provider == session.provider. (Evaluated before `session` is moved.)
        tab_id: session.id.clone(),
        provider: session.provider.clone(),
        session,
        provider_config: provider_cfg,
        env,
        identity,
        resume: launch_with_resume,
        pty_size: (rows, cols),
        scrollback_lines: config.ui.agent_scrollback_lines,
        kind: AgentLaunchKind::Create {
            status_message,
            repo_path: repo_path.to_string_lossy().to_string(),
            owns_worktree,
            startup_result,
            status_op_id: create_key.clone(),
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

    let client = match crate::pty::PtyClient::spawn_with_env_opts(
        &request.provider_config.command,
        &launch_args,
        Path::new(&request.session.worktree_path),
        rows,
        cols,
        request.scrollback_lines,
        crate::pty::PtySpawnOptions {
            env: &request.env,
            track_agent_signals: true,
            identity: &request.identity,
        },
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::mpsc;

    use super::*;
    use crate::model::{Project, ProjectBranchStatus, ProviderKind};

    /// Initialize a throwaway git repo with a single commit on `main`.
    fn init_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str]| {
            let out = crate::git::test_support::git_command()
                .args(args)
                .current_dir(p)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        run(&["commit", "--allow-empty", "-m", "init"]);
        dir
    }

    fn test_project(repo: &Path) -> Project {
        Project {
            id: "proj-1".to_string(),
            name: "repo".to_string(),
            path: repo.to_string_lossy().to_string(),
            explicit_default_provider: None,
            // `provider_config` falls back to the provider name as the command,
            // so a provider literally named "cat" spawns `cat` — a harmless PTY
            // process that stays alive on stdin, available on any Unix PATH.
            default_provider: ProviderKind::new("cat"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    /// Everything a create job emitted, for asserting on success, failure,
    /// the final status message, and the progress trail alike.
    struct JobRun {
        session: Option<AgentSession>,
        status_message: Option<String>,
        failure: Option<String>,
        progress: Vec<String>,
        /// Keeps the temporary worktrees root alive so tests can inspect the
        /// created worktree's contents.
        _paths_root: tempfile::TempDir,
    }

    /// Drive `run_create_agent_job` for an arbitrary request against `repo`
    /// and collect every event the job emitted.
    fn drive_create_job_run(repo: &Path, request: CreateAgentRequest) -> JobRun {
        let paths_root = tempfile::tempdir().unwrap();
        let paths = DuxPaths {
            root: paths_root.path().to_path_buf(),
            config_path: paths_root.path().join("config.toml"),
            sessions_db_path: paths_root.path().join("sessions.sqlite3"),
            worktrees_root: paths_root.path().join("worktrees"),
            lock_path: paths_root.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();

        let _ = repo; // repo is referenced by the request; kept alive by caller.
        let (tx, rx) = mpsc::channel();
        run_create_agent_job(
            request,
            paths,
            Config::default(),
            tx,
            (80, 24),
            "op-1".to_string(),
            crate::term_identity::TerminalIdentity::default(),
        );
        let mut run = JobRun {
            session: None,
            status_message: None,
            failure: None,
            progress: Vec::new(),
            _paths_root: paths_root,
        };
        while let Ok(event) = rx.try_recv() {
            match event {
                WorkerEvent::AgentLaunchReady(data) => {
                    run.session = Some(data.request.session.clone());
                    if let AgentLaunchKind::Create { status_message, .. } = &data.request.kind {
                        run.status_message = Some(status_message.clone());
                    }
                }
                WorkerEvent::CreateAgentFailed { message, .. } => {
                    run.failure = Some(message);
                }
                WorkerEvent::CreateAgentProgress { message, .. } => {
                    run.progress.push(message);
                }
                _ => {}
            }
        }
        run
    }

    /// Drive `run_create_agent_job` and return the `AgentSession` it
    /// constructed. Panics with the failure message if the job emits
    /// `CreateAgentFailed` instead.
    fn drive_create_job(repo: &Path, request: CreateAgentRequest) -> AgentSession {
        let run = drive_create_job_run(repo, request);
        if let Some(message) = run.failure {
            panic!("create job failed: {message}");
        }
        run.session
            .expect("the job should emit an AgentLaunchReady with the session")
    }

    /// Drive `run_create_agent_job` for a `NewProject` request and return the
    /// `AgentSession` the job constructed.
    fn create_session_for(custom_name: Option<String>) -> AgentSession {
        let repo = init_test_repo();
        let project = test_project(repo.path());
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        drive_create_job(repo.path(), request)
    }

    /// Create a branch `name` (pointing at HEAD) in `repo` so an "attach to
    /// existing branch" path can find it.
    fn create_branch(repo: &Path, name: &str) {
        let out = crate::git::test_support::git_command()
            .args(["branch", name])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git branch {name} failed");
    }

    /// A minimal `AgentSession` rooted at `worktree` (a real git worktree so
    /// `head_commit`/`copy_uncommitted_changes` succeed), for the fork arms.
    fn fork_source_session(worktree: &Path) -> AgentSession {
        AgentSession {
            id: "src-1".to_string(),
            project_id: "proj-1".to_string(),
            project_path: None,
            provider: ProviderKind::new("cat"),
            source_branch: "main".to_string(),
            branch_name: "src-branch".to_string(),
            initial_branch: "src-branch".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
        }
    }

    #[test]
    fn pull_request_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        // Attach to an existing local branch so the arm avoids the network fetch.
        create_branch(repo.path(), "pr-agent");
        let request = CreateAgentRequest::PullRequest {
            project: test_project(repo.path()),
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            number: 42,
            title: "Fix the bug".to_string(),
            state: "OPEN".to_string(),
            head_branch: "pr-head".to_string(),
            custom_name: Some("pr-agent".to_string()),
            use_existing_branch: true,
        };
        let session = drive_create_job(repo.path(), request);
        // The typed name is durable identity (title) and names the branch; the
        // birth branch is recorded immutably and equals the created branch.
        assert_eq!(session.title.as_deref(), Some("pr-agent"));
        assert_eq!(session.branch_name, "pr-agent");
        assert_eq!(session.initial_branch, "pr-agent");
    }

    #[test]
    fn fork_session_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        // The source worktree is the repo itself (a git dir with a HEAD commit).
        let source = fork_source_session(repo.path());
        let request = CreateAgentRequest::ForkSession {
            project: test_project(repo.path()),
            source_session: Box::new(source),
            source_label: "src agent".to_string(),
            custom_name: Some("forked-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(session.title.as_deref(), Some("forked-agent"));
        assert_eq!(session.branch_name, "forked-agent");
        assert_eq!(session.initial_branch, "forked-agent");
    }

    #[test]
    fn fork_external_worktree_arm_sets_title_and_initial_branch_from_typed_name() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ForkExternalWorktree {
            project: test_project(repo.path()),
            source_worktree_path: repo.path().to_path_buf(),
            source_label: "ext worktree".to_string(),
            source_branch: "main".to_string(),
            custom_name: Some("external-agent".to_string()),
        };
        let session = drive_create_job(repo.path(), request);
        assert_eq!(session.title.as_deref(), Some("external-agent"));
        assert_eq!(session.branch_name, "external-agent");
        assert_eq!(session.initial_branch, "external-agent");
    }

    #[test]
    fn fork_external_worktree_arm_without_name_keeps_title_none() {
        let repo = init_test_repo();
        let request = CreateAgentRequest::ForkExternalWorktree {
            project: test_project(repo.path()),
            source_worktree_path: repo.path().to_path_buf(),
            source_label: "ext worktree".to_string(),
            source_branch: "main".to_string(),
            custom_name: None,
        };
        let session = drive_create_job(repo.path(), request);
        // No typed name: title stays None, but the auto-derived branch is still
        // recorded as the immutable initial branch.
        assert_eq!(session.title, None);
        assert_eq!(session.initial_branch, session.branch_name);
        assert!(!session.branch_name.is_empty());
    }

    #[test]
    fn a_named_new_agent_stores_the_typed_name_as_title() {
        let session = create_session_for(Some("server-mode".to_string()));
        // The typed name is durable identity (title), and it also names the branch.
        assert_eq!(session.title.as_deref(), Some("server-mode"));
        assert_eq!(session.branch_name, "server-mode");
        // The birth branch is recorded immutably and equals the created branch.
        assert_eq!(session.initial_branch, "server-mode");
    }

    #[test]
    fn an_auto_named_agent_keeps_title_none() {
        let session = create_session_for(None);
        // An auto pet-name leaves title empty so the display keeps tracking the
        // branch, but the pet name still becomes the immutable initial branch.
        assert_eq!(session.title, None);
        assert_eq!(session.initial_branch, session.branch_name);
        assert!(!session.branch_name.is_empty());
    }

    // ── uncommitted-changes copy and best-effort pull ────────────

    fn git_in(cwd: &Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn new_project_request(
        repo: &Path,
        pull_before_create: bool,
        copy_uncommitted_changes: bool,
    ) -> CreateAgentRequest {
        CreateAgentRequest::NewProject {
            project: test_project(repo),
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create,
            copy_uncommitted_changes,
        }
    }

    /// Happy path: the checkout's dirt travels; gitignored files do not.
    #[test]
    fn fresh_agent_copies_uncommitted_changes_from_checkout() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        std::fs::write(repo.path().join("junk.log"), "ignored\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), false, true));
        assert!(run.failure.is_none(), "creation must succeed");
        let worktree = PathBuf::from(&run.session.as_ref().unwrap().worktree_path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "dirty\n"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!worktree.join("junk.log").exists());
    }

    /// PROVEN DEFECT: with the checkout on a different commit than the new
    /// worktree, applying the status delta would delete files the worktree
    /// legitimately has. The HEAD guard must skip the copy with a note.
    #[test]
    fn fresh_agent_skips_copy_when_checkout_head_differs() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("keep.txt"), "on main\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);
        // Park the checkout on `feature` at a DIFFERENT commit, with an
        // uncommitted `rm` of a file that exists on `main`.
        git_in(repo.path(), &["switch", "-c", "feature"]);
        std::fs::write(repo.path().join("feature-only.txt"), "feature\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "feature commit"]);
        std::fs::remove_file(repo.path().join("keep.txt")).unwrap();

        let mut project = test_project(repo.path());
        project.current_branch = "feature".to_string();
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none(), "creation must succeed");
        let session = run.session.unwrap();
        let worktree = PathBuf::from(&session.worktree_path);
        assert!(
            worktree.join("keep.txt").exists(),
            "the uncommitted rm must NOT be applied across different commits"
        );
        let status = run.status_message.unwrap();
        assert!(
            status.contains("were not copied"),
            "the skip must be visible in the status message, got: {status}"
        );
    }

    /// Ticking "copy my uncommitted changes" must not move the user's branch:
    /// with the pull off there is no switch at all.
    #[test]
    fn copy_only_creation_does_not_switch_the_shared_checkout() {
        let repo = init_test_repo();
        git_in(repo.path(), &["switch", "-c", "feature"]);
        assert_eq!(
            git_stdout(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "feature"
        );

        let mut project = test_project(repo.path());
        project.current_branch = "feature".to_string();
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none(), "creation must succeed");
        assert_eq!(
            git_stdout(repo.path(), &["symbolic-ref", "--short", "HEAD"]),
            "feature",
            "a copy-only creation must not switch the shared checkout"
        );
    }

    #[test]
    fn fresh_agent_with_copy_disabled_copies_nothing() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), false, false));
        assert!(run.failure.is_none(), "creation must succeed");
        let worktree = PathBuf::from(&run.session.as_ref().unwrap().worktree_path);
        assert!(!worktree.join("note.txt").exists());
    }

    /// Kills the old hard abort: a failed pull is a warning note, not a
    /// creation failure.
    #[test]
    fn fresh_agent_creation_survives_pull_failure() {
        let repo = init_test_repo();
        git_in(
            repo.path(),
            &["remote", "add", "origin", "/nonexistent/dux-test-origin"],
        );

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, false));
        assert!(run.failure.is_none(), "creation must survive a failed pull");
        let status = run.status_message.unwrap();
        assert!(
            status.contains("could not pull"),
            "the pull failure must be visible in the status message, got: {status}"
        );
    }

    /// Kills the blanket dirty abort: a dirty checkout no longer blocks the
    /// pull, and git itself fast-forwards when nothing conflicts.
    #[test]
    fn fresh_agent_creation_survives_dirty_checkout_and_still_pulls() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "base"]);

        // A bare origin one commit ahead on an unrelated file.
        let bare = tempfile::tempdir().unwrap();
        git_in(bare.path(), &["init", "--bare", "-b", "main"]);
        git_in(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git_in(repo.path(), &["push", "origin", "main"]);
        let staging = tempfile::tempdir().unwrap();
        git_in(
            staging.path(),
            &["clone", bare.path().to_str().unwrap(), "."],
        );
        git_in(staging.path(), &["config", "user.name", "test"]);
        git_in(staging.path(), &["config", "user.email", "t@t"]);
        std::fs::write(staging.path().join("upstream.txt"), "ahead\n").unwrap();
        git_in(staging.path(), &["add", "-A"]);
        git_in(staging.path(), &["commit", "-m", "upstream"]);
        git_in(staging.path(), &["push", "origin", "main"]);

        // A tracked local edit that would have tripped the old dirty gate.
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, true));
        assert!(
            run.failure.is_none(),
            "a dirty checkout must not block creation: {:?}",
            run.failure
        );
        let session = run.session.unwrap();
        let worktree = PathBuf::from(&session.worktree_path);
        assert!(
            worktree.join("upstream.txt").exists(),
            "the pull must have fast-forwarded"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "dirty\n",
            "the local edit still travels"
        );
    }

    /// The local-only flagship case: no origin means a log-only pull skip
    /// (no warning), and the copy still runs.
    #[test]
    fn fresh_agent_skips_pull_without_origin_and_still_copies() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();

        let run = drive_create_job_run(repo.path(), new_project_request(repo.path(), true, true));
        assert!(run.failure.is_none());
        let session = run.session.unwrap();
        let worktree = PathBuf::from(&session.worktree_path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        let status = run.status_message.unwrap();
        assert!(
            !status.contains("Warning"),
            "a missing origin is steady state, not a warning: {status}"
        );
    }

    /// No per-path exceptions: attaching to an existing branch copies when
    /// the checkout and the branch are on the same commit, and skips with a
    /// note when they are not.
    #[test]
    fn attach_existing_branch_copies_when_heads_match_and_skips_when_not() {
        // Same tip: the dirt travels.
        let repo = init_test_repo();
        create_branch(repo.path(), "same-tip");
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("same-tip".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none());
        let worktree = PathBuf::from(&run.session.unwrap().worktree_path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );

        // Different tip: skipped with a visible note.
        let repo = init_test_repo();
        create_branch(repo.path(), "other-tip");
        git_in(repo.path(), &["commit", "--allow-empty", "-m", "advance"]);
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let request = CreateAgentRequest::NewProject {
            project: test_project(repo.path()),
            custom_name: Some("other-tip".to_string()),
            use_existing_branch: true,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(run.failure.is_none());
        let worktree = PathBuf::from(&run.session.unwrap().worktree_path);
        assert!(!worktree.join("note.txt").exists());
        let status = run.status_message.unwrap();
        assert!(
            status.contains("were not copied"),
            "the skip must be visible: {status}"
        );
    }

    /// The fork rule change: gitignored files no longer travel on forks.
    #[test]
    fn fork_copy_excludes_gitignored_files() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join(".gitignore"), "*.log\n").unwrap();
        git_in(repo.path(), &["add", "-A"]);
        git_in(repo.path(), &["commit", "-m", "gitignore"]);
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        std::fs::write(repo.path().join("junk.log"), "ignored\n").unwrap();

        let source = fork_source_session(repo.path());
        let request = CreateAgentRequest::ForkSession {
            project: test_project(repo.path()),
            source_session: Box::new(source),
            source_label: "src agent".to_string(),
            custom_name: Some("forked-agent".to_string()),
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(
            run.failure.is_none(),
            "the fork must succeed: {:?}",
            run.failure
        );
        let worktree = PathBuf::from(&run.session.as_ref().unwrap().worktree_path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("note.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!worktree.join("junk.log").exists());
    }

    /// Ordering: the copy runs AFTER the provider availability check, so an
    /// unavailable provider fails before any copy progress is reported.
    #[test]
    fn provider_failure_after_worktree_creation_does_not_report_copied_then_discard() {
        let repo = init_test_repo();
        std::fs::write(repo.path().join("note.txt"), "untracked\n").unwrap();
        let mut project = test_project(repo.path());
        project.default_provider = ProviderKind::new("definitely-not-a-real-command-dux");
        let request = CreateAgentRequest::NewProject {
            project,
            custom_name: Some("copy-target".to_string()),
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: true,
        };
        let run = drive_create_job_run(repo.path(), request);
        assert!(
            run.failure.is_some(),
            "the unavailable provider must fail the job"
        );
        assert!(
            !run.progress
                .iter()
                .any(|message| message.contains("Copying uncommitted")),
            "no copy progress may be reported before the provider check: {:?}",
            run.progress
        );
    }
}
