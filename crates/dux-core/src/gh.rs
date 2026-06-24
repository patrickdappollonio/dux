//! GitHub CLI (`gh`) integration helpers used by the PR-sync worker
//! (`spawn_pr_sync_worker`, `spawn_initial_pr_refresh`, `spawn_pr_check_for_session`).
//! All helpers shell out to `gh` and parse JSON; no UI deps.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::git;
use crate::logger;
use crate::model::{PrInfo, PrState, Project};
use crate::storage::StoredPr;
use crate::worker::{PrSyncEntry, PullRequestLookup, ResolvedPullRequest, WorkerEvent};

pub fn run_pr_sync(sessions: &Arc<Mutex<Vec<PrSyncEntry>>>) -> Vec<(String, Option<PrInfo>)> {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return Vec::new(),
    };
    snapshot
        .iter()
        .map(|entry| {
            let result = check_pr_for_entry(entry);
            (entry.session_id.clone(), result)
        })
        .collect()
}

/// Determine the current PR state for a session. The check strategy depends on
/// what we already know and whether the agent is still running:
///
/// | Known PR state | Agent running? | Action                                |
/// |----------------|---------------|---------------------------------------|
/// | None           | any           | `gh pr list --head` to discover       |
/// | OPEN           | any           | `gh pr view` + discover newer         |
/// | MERGED/CLOSED  | yes           | discover newer (agent may push again) |
/// | MERGED/CLOSED  | no            | **zero calls** — nothing will change  |
///
/// The last row is the key optimization: once a PR is in a terminal state and
/// the agent has exited, nobody is pushing to that branch anymore, so there is
/// no reason to check for newer PRs. This reduces API calls from O(sessions)
/// to O(active_sessions) for repos with many completed agents.
pub fn check_pr_for_entry(entry: &PrSyncEntry) -> Option<PrInfo> {
    let remote = git::remote_github_repo(Path::new(&entry.worktree_path));
    let (host, owner_repo) = if let Some(remote) = remote {
        (remote.host, remote.owner_repo)
    } else if let Some(known) = &entry.known_pr {
        (known.host.clone(), known.owner_repo.clone())
    } else {
        return None;
    };

    if let Some(ref known) = entry.known_pr {
        let is_terminal = known.state == "MERGED" || known.state == "CLOSED";

        if is_terminal {
            if entry.agent_exited {
                // Terminal PR + exited agent = zero network calls.
                // The agent process is gone and the PR is already merged/closed,
                // so no new commits or PRs will appear on this branch.
                return reconstruct_from_stored(known);
            }

            // Terminal PR but agent is still running — it might push new commits
            // and open a follow-up PR, so we still check for newer PRs.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
                && newer.number > known.pr_number
            {
                return Some(newer);
            }
            return reconstruct_from_stored(known);
        }

        // Open PR: refresh its current state via `gh pr view`.
        if let Some(pr) = view_pr_by_number(
            known.pr_number,
            &known.host,
            &known.owner_repo,
            &entry.session_id,
        ) {
            // Also check if a newer PR was opened.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
                && newer.number > pr.number
            {
                return Some(newer);
            }
            return Some(pr);
        }
    }

    // No known PR — discover by branch name.
    discover_pr_by_branch(&entry.branch_name, &host, &owner_repo, &entry.session_id)
}

/// Reconstruct a PrInfo from stored data without a network call.
/// Used for terminal states (merged/closed) that don't need refreshing.
fn reconstruct_from_stored(stored: &StoredPr) -> Option<PrInfo> {
    let state = match stored.state.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        "OPEN" => PrState::Open,
        _ => return None,
    };
    Some(PrInfo {
        number: stored.pr_number,
        state,
        title: stored.title.clone(),
        host: stored.host.clone(),
        owner_repo: stored.owner_repo.clone(),
        url: stored.url.clone(),
    })
}

/// Check a known PR by number using `gh pr view`.
fn view_pr_by_number(
    number: u64,
    host: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<PrInfo> {
    let repo = gh_repo_arg(host, owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            &repo,
            "--json",
            "number,state,title,url",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr view #{number} failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_pr_json_object(text.trim(), host, owner_repo)
}

/// Discover a PR by branch name using `gh pr list --state all`.
fn discover_pr_by_branch(
    branch: &str,
    host: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<PrInfo> {
    let repo = gh_repo_arg(host, owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--repo",
            &repo,
            "--state",
            "all",
            "--json",
            "number,state,title,url",
            "--limit",
            "1",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr list failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(text.trim()).ok()?;
    let obj = arr.first()?;
    parse_pr_json_value(obj, host, owner_repo)
}

/// Parse a single PR JSON object (from `gh pr view` output).
fn parse_pr_json_object(json: &str, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, host, owner_repo)
}

/// Extract PrInfo from a serde_json::Value.
fn parse_pr_json_value(obj: &serde_json::Value, host: &str, owner_repo: &str) -> Option<PrInfo> {
    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| pull_request_url(host, owner_repo, number));
    let state = match state_str {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };

    Some(PrInfo {
        number,
        state,
        title,
        host: normalize_github_host(host).to_string(),
        owner_repo: owner_repo.to_string(),
        url,
    })
}

pub fn pull_request_url(host: &str, owner_repo: &str, number: u64) -> String {
    let host = normalize_github_host(host);
    format!("https://{host}/{owner_repo}/pull/{number}")
}

pub fn gh_repo_arg(host: &str, owner_repo: &str) -> String {
    let host = normalize_github_host(host);
    if host == "github.com" {
        owner_repo.to_string()
    } else {
        format!("{host}/{owner_repo}")
    }
}

fn normalize_github_host(host: &str) -> &str {
    if host.trim().is_empty() {
        "github.com"
    } else {
        host
    }
}

/// Parse a user-typed PR reference into a [`PullRequestLookup`] for the selected
/// project's GitHub remote. Accepts:
///   - a bare PR number (`123`) — assumes the selected project's host/repo,
///   - a `#`-prefixed number (`#123`) — same assumption,
///   - a full GitHub PR URL (`https://github.com/owner/repo/pull/123`,
///     including GitHub Enterprise hosts and trailing `?query`/`#fragment`).
///
/// A URL whose host or owner/repo does not match the selected project's remote
/// is rejected with an actionable error, since fetching another repo's PR head
/// into this project's worktree would silently do the wrong thing.
///
/// This is a pure function shared by the TUI's new-agent-from-pr prompt and the
/// web's `CreateAgentFromPr` wire flow.
pub fn parse_pull_request_lookup(
    raw_input: &str,
    selected_host: &str,
    selected_owner_repo: &str,
) -> Result<PullRequestLookup, String> {
    let input = raw_input.trim();
    if input.is_empty() {
        return Err("Enter a GitHub PR URL or PR number.".to_string());
    }

    if let Ok(number) = input.strip_prefix('#').unwrap_or(input).parse::<u64>() {
        return Ok(PullRequestLookup {
            host: selected_host.to_string(),
            owner_repo: selected_owner_repo.to_string(),
            number,
        });
    }

    let Some((host, rest)) = parse_github_pull_url_parts(input) else {
        return Err("Enter a PR number, #number, or a GitHub PR URL.".to_string());
    };
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() < 4 || parts[2] != "pull" {
        return Err(
            "GitHub PR URLs must look like https://github.com/owner/repo/pull/123.".to_string(),
        );
    }
    let owner_repo = format!("{}/{}", parts[0], parts[1]);
    if !host.eq_ignore_ascii_case(selected_host)
        || !owner_repo.eq_ignore_ascii_case(selected_owner_repo)
    {
        return Err(format!(
            "PR belongs to {host}/{owner_repo}, but the selected project uses {selected_host}/{selected_owner_repo}."
        ));
    }
    let number = parts[3]
        .parse::<u64>()
        .map_err(|_| "GitHub PR URL does not contain a valid PR number.".to_string())?;
    Ok(PullRequestLookup {
        host,
        owner_repo,
        number,
    })
}

fn parse_github_pull_url_parts(input: &str) -> Option<(String, String)> {
    let without_scheme = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let (host, rest) = without_scheme.split_once('/')?;
    if host != "github.com" && !host.starts_with("github.") {
        return None;
    }
    let rest = rest
        .split(['?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_end_matches('/')
        .to_string();
    Some((host.to_string(), rest))
}

/// Resolve a PR reference for a project against the GitHub remote and `gh` CLI,
/// posting [`WorkerEvent::PullRequestResolved`] with the outcome. Runs on a
/// background thread (it parses the project remote, then shells out to
/// `gh pr view`). Shared by the TUI's `dispatch_pull_request_lookup` and the
/// web's `CreateAgentFromPr` flow so both surfaces resolve PRs identically.
///
/// `custom_name` carries a caller-supplied display name through to the resolved
/// PR (`None` for the TUI, which prompts for a name after resolution; `Some` for
/// the web, which sends the name upfront).
pub fn run_pull_request_lookup_job(
    project: Project,
    raw_input: String,
    custom_name: Option<String>,
    worker_tx: Sender<WorkerEvent>,
    status_op_id: Option<String>,
) {
    let lookup = match git::remote_github_repo(Path::new(&project.path)) {
        Some(remote) => parse_pull_request_lookup(&raw_input, &remote.host, &remote.owner_repo),
        None => Err(format!(
            "Project \"{}\" does not have a GitHub origin remote.",
            project.name
        )),
    };
    let lookup = match lookup {
        Ok(lookup) => lookup,
        Err(message) => {
            let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
                result: Err(message),
                status_op_id,
            });
            return;
        }
    };

    let repo = gh_repo_arg(&lookup.host, &lookup.owner_repo);
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &lookup.number.to_string(),
            "--repo",
            &repo,
            "--json",
            "number,title,state,headRefName",
        ])
        .output();
    let result = match output {
        Ok(output) if output.status.success() => parse_resolved_pull_request_json(
            &String::from_utf8_lossy(&output.stdout),
            project,
            &lookup.host,
            &lookup.owner_repo,
            custom_name,
        ),
        Ok(output) => Err(format!(
            "Failed to resolve PR #{} from {}: {}",
            lookup.number,
            lookup.owner_repo,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(err) => Err(format!("Failed to run gh pr view: {err}")),
    };
    let _ = worker_tx.send(WorkerEvent::PullRequestResolved {
        result,
        status_op_id,
    });
}

fn parse_resolved_pull_request_json(
    json: &str,
    project: Project,
    host: &str,
    owner_repo: &str,
    custom_name: Option<String>,
) -> Result<ResolvedPullRequest, String> {
    let obj: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|err| format!("gh returned invalid PR JSON: {err}"))?;
    let number = obj
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "gh PR response did not include a PR number.".to_string())?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = obj
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let head_ref_name = obj
        .get("headRefName")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "gh PR response did not include a head branch.".to_string())?
        .to_string();
    Ok(ResolvedPullRequest {
        project,
        host: host.to_string(),
        owner_repo: owner_repo.to_string(),
        number,
        title,
        state,
        head_ref_name,
        custom_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gh_repo_arg_uses_owner_repo_for_github_dot_com() {
        assert_eq!(gh_repo_arg("github.com", "owner/repo"), "owner/repo");
        assert_eq!(gh_repo_arg("", "owner/repo"), "owner/repo");
    }

    #[test]
    fn gh_repo_arg_includes_host_for_enterprise() {
        assert_eq!(
            gh_repo_arg("github.example.com", "owner/repo"),
            "github.example.com/owner/repo"
        );
    }

    #[test]
    fn pull_request_url_defaults_empty_host_to_github_dot_com() {
        assert_eq!(
            pull_request_url("", "owner/repo", 12),
            "https://github.com/owner/repo/pull/12"
        );
    }

    #[test]
    fn parse_pr_json_object_uses_gh_url_when_present() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"OPEN","title":"Demo","url":"https://github.com/owner/repo/pull/42"}"#,
            "github.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
    }

    #[test]
    fn parse_pr_json_object_falls_back_to_host_url() {
        let pr = parse_pr_json_object(
            r#"{"number":42,"state":"MERGED","title":"Demo"}"#,
            "github.example.com",
            "owner/repo",
        )
        .expect("pr");

        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.url, "https://github.example.com/owner/repo/pull/42");
    }

    #[test]
    fn parse_pull_request_lookup_accepts_number_and_hash_number() {
        let plain = parse_pull_request_lookup("123", "github.com", "octocat/Hello-World")
            .expect("plain number");
        assert_eq!(plain.host, "github.com");
        assert_eq!(plain.owner_repo, "octocat/Hello-World");
        assert_eq!(plain.number, 123);

        let hashed = parse_pull_request_lookup("#456", "github.example.com", "octocat/Hello-World")
            .expect("hash number");
        assert_eq!(hashed.host, "github.example.com");
        assert_eq!(hashed.owner_repo, "octocat/Hello-World");
        assert_eq!(hashed.number, 456);
    }

    #[test]
    fn parse_pull_request_lookup_accepts_matching_github_url() {
        let lookup = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/pull/789?foo=bar",
            "github.com",
            "octocat/Hello-World",
        )
        .expect("matching URL");
        assert_eq!(lookup.host, "github.com");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 789);
    }

    #[test]
    fn parse_pull_request_lookup_accepts_matching_enterprise_url() {
        let lookup = parse_pull_request_lookup(
            "https://github.example.com/octocat/Hello-World/pull/789",
            "github.example.com",
            "octocat/Hello-World",
        )
        .expect("matching enterprise URL");
        assert_eq!(lookup.host, "github.example.com");
        assert_eq!(lookup.owner_repo, "octocat/Hello-World");
        assert_eq!(lookup.number, 789);
    }

    #[test]
    fn parse_pull_request_lookup_strips_trailing_slash_and_fragment() {
        let lookup = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/pull/5/#discussion",
            "github.com",
            "octocat/Hello-World",
        )
        .expect("trailing slash + fragment");
        assert_eq!(lookup.number, 5);
    }

    #[test]
    fn parse_pull_request_lookup_rejects_mismatched_github_url() {
        let err = parse_pull_request_lookup(
            "https://github.com/other/repo/pull/12",
            "github.com",
            "octocat/Hello-World",
        )
        .expect_err("mismatched repo");
        assert!(err.contains("selected project uses github.com/octocat/Hello-World"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_empty_input() {
        let err = parse_pull_request_lookup("   ", "github.com", "octocat/Hello-World")
            .expect_err("empty");
        assert!(err.contains("Enter a GitHub PR URL or PR number"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_garbage() {
        let err = parse_pull_request_lookup("not-a-pr", "github.com", "octocat/Hello-World")
            .expect_err("garbage");
        assert!(err.contains("Enter a PR number, #number, or a GitHub PR URL"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_non_github_url() {
        let err = parse_pull_request_lookup(
            "https://gitlab.com/octocat/Hello-World/pull/1",
            "github.com",
            "octocat/Hello-World",
        )
        .expect_err("non-github host");
        assert!(err.contains("Enter a PR number, #number, or a GitHub PR URL"));
    }

    #[test]
    fn parse_pull_request_lookup_rejects_malformed_pull_path() {
        let err = parse_pull_request_lookup(
            "https://github.com/octocat/Hello-World/issues/3",
            "github.com",
            "octocat/Hello-World",
        )
        .expect_err("not a pull URL");
        assert!(err.contains("must look like https://github.com/owner/repo/pull/123"));
    }

    fn lookup_test_project() -> Project {
        Project {
            id: "p1".to_string(),
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            explicit_default_provider: None,
            default_provider: crate::model::ProviderKind::new("claude"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: std::collections::BTreeMap::new(),
            current_branch: "main".to_string(),
            branch_status: crate::model::ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    #[test]
    fn parse_resolved_pull_request_json_extracts_fields() {
        let resolved = parse_resolved_pull_request_json(
            r#"{"number":42,"title":"Fix bug","state":"OPEN","headRefName":"feature/fix"}"#,
            lookup_test_project(),
            "github.com",
            "octocat/Hello-World",
            Some("my-name".to_string()),
        )
        .expect("resolved");
        assert_eq!(resolved.number, 42);
        assert_eq!(resolved.title, "Fix bug");
        assert_eq!(resolved.state, "OPEN");
        assert_eq!(resolved.head_ref_name, "feature/fix");
        assert_eq!(resolved.host, "github.com");
        assert_eq!(resolved.owner_repo, "octocat/Hello-World");
        assert_eq!(resolved.custom_name.as_deref(), Some("my-name"));
    }

    #[test]
    fn parse_resolved_pull_request_json_rejects_missing_head_branch() {
        let err = parse_resolved_pull_request_json(
            r#"{"number":42,"title":"Fix bug","state":"OPEN"}"#,
            lookup_test_project(),
            "github.com",
            "octocat/Hello-World",
            None,
        )
        .expect_err("missing head");
        assert!(err.contains("head branch"));
    }
}
