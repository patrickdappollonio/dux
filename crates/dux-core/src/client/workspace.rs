//! `dux projects`, `agents`, `agents tabs`, `agents prs`, `projects worktrees` and
//! `terminals`: the resources kept in `sessions.sqlite3` and the running
//! engine, so every one of them goes through a running dux.
//!
//! A read builds its list from the API's own reads, in their order. A change
//! is planned first (the resource it names is looked up, and the question to
//! ask is worded), then asked about by the caller, then sent and waited for
//! by [`perform`].

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use super::connect::Client;
use super::output::{self, Listing, Row, Shape};
use super::transport::Method;
use super::wait::{self, OperationRecord, RecordState, segment};
use super::{CliError, Exit};
use crate::operations::CREATE_IN_FLIGHT_REFUSAL;
use crate::pr_reference::TypedReference;
use crate::prose::Prose;
use crate::status_text::ReviewCopyOutlook;

// ---------------------------------------------------------------------------
// What the API answers
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize)]
struct Project {
    id: String,
    name: String,
    path: String,
    #[serde(default)]
    current_branch: String,
    #[serde(default)]
    branch_status: String,
    #[serde(default)]
    leading_branch: Option<String>,
    #[serde(default)]
    default_provider: String,
    #[serde(default)]
    startup_command: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    path_missing: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Workspace {
    Managed {
        project_id: String,
        branch_name: String,
        worktree_path: String,
    },
    Folder {
        folder_path: String,
        folder_label: String,
    },
}

#[derive(Clone, Debug, Deserialize)]
struct Pr {
    number: u64,
    state: String,
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    owner_repo: String,
    #[serde(default)]
    own_repo: bool,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    found: bool,
    #[serde(default)]
    attached: bool,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl Pr {
    /// How the lists name it: `#418` in the agent's own repository,
    /// `acme/infra#77` in any other.
    fn label(&self) -> String {
        if self.own_repo {
            format!("#{}", self.number)
        } else {
            format!("{}#{}", self.owner_repo, self.number)
        }
    }

    /// The branch of the agent's own repository it merges from, once GitHub
    /// has said; another repository's branch is none of the agent's.
    fn branch(&self) -> Option<&str> {
        self.head_branch.as_deref().filter(|_| self.own_repo)
    }

    /// Whether dux found it, the user attached it, or both.
    fn source(&self) -> &'static str {
        match (self.found, self.attached) {
            (true, true) => "found+attached",
            (false, true) => "attached",
            _ => "found",
        }
    }

    /// The one line `dux agents show` and `ls --format json` name the shown
    /// pull request with.
    fn shown_line(&self) -> String {
        format!("#{} {} {}", self.number, self.state, self.url)
    }

    /// One entry of `dux agents show`'s lists: its name, state, where it
    /// comes from (its branch, else `attached`) and its link.
    fn list_line(&self) -> String {
        let from = self
            .branch()
            .or_else(|| self.attached.then_some("attached"));
        match from {
            Some(from) => format!("{} {} {from} {}", self.label(), self.state, self.url),
            None => format!("{} {} {}", self.label(), self.state, self.url),
        }
    }

    /// Whether it is the pull request `key` names; host and repository
    /// compare without regard to case, as GitHub's do.
    fn is(&self, key: &PrKeyAnswer) -> bool {
        self.number == key.number
            && self.host.eq_ignore_ascii_case(&key.host)
            && self.owner_repo.eq_ignore_ascii_case(&key.owner_repo)
    }

    /// What a change names it by.
    fn key(&self) -> Value {
        json!({ "host": self.host, "owner_repo": self.owner_repo, "number": self.number })
    }
}

/// A pull request as the branch list names it.
#[derive(Clone, Debug, Deserialize)]
struct PrKeyAnswer {
    host: String,
    owner_repo: String,
    number: u64,
}

/// One branch an agent's worktree has been on.
#[derive(Clone, Debug, Deserialize)]
struct BranchAnswer {
    name: String,
    #[serde(default)]
    former_names: Vec<String>,
    #[serde(default)]
    current: bool,
    #[serde(default)]
    first_seen_at: Option<String>,
    #[serde(default)]
    prs: Vec<PrKeyAnswer>,
}

/// `GET …/pull-requests/branches`.
#[derive(Clone, Debug, Deserialize)]
struct BranchesAnswer {
    #[serde(default)]
    branches: Vec<BranchAnswer>,
    #[serde(default)]
    attached_on_no_branch: Vec<PrKeyAnswer>,
}

#[derive(Clone, Debug, Deserialize)]
struct Tab {
    id: String,
    provider: String,
    #[serde(default)]
    has_live_process: bool,
    #[serde(default)]
    working: bool,
    #[serde(default)]
    needs_attention: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Owner {
    Session { session_id: String },
    Project { project_id: String },
    Standalone { cwd_label: String },
}

#[derive(Clone, Debug, Deserialize)]
struct Terminal {
    id: String,
    owner: Owner,
    label: String,
    #[serde(default)]
    foreground_cmd: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Agent {
    id: String,
    #[serde(default)]
    title: Option<String>,
    provider: String,
    workspace: Workspace,
    status: String,
    #[serde(default)]
    pr: Option<Pr>,
    #[serde(default)]
    prs: Vec<Pr>,
    #[serde(default)]
    prs_hidden: Vec<Pr>,
    #[serde(default)]
    pr_sort: String,
    #[serde(default)]
    tabs: Vec<Tab>,
    #[serde(default)]
    terminals: Vec<Terminal>,
    #[serde(default)]
    removing: bool,
    #[serde(default)]
    remote_viewers: Option<usize>,
}

impl Agent {
    /// The agent's name, by the rule both surfaces show it with: its title,
    /// else its branch, else its folder's own name.
    fn name(&self) -> String {
        if let Some(title) = &self.title {
            return title.clone();
        }
        match &self.workspace {
            Workspace::Managed { branch_name, .. } => branch_name.clone(),
            Workspace::Folder { folder_path, .. } => {
                Path::new(folder_path).file_name().map_or_else(
                    || folder_path.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )
            }
        }
    }

    fn project_id(&self) -> Option<&str> {
        match &self.workspace {
            Workspace::Managed { project_id, .. } => Some(project_id),
            Workspace::Folder { .. } => None,
        }
    }

    /// Where the agent works: its worktree, or the folder it was started in.
    fn directory(&self) -> &str {
        match &self.workspace {
            Workspace::Managed { worktree_path, .. } => worktree_path,
            Workspace::Folder { folder_path, .. } => folder_path,
        }
    }

    fn state(&self) -> &str {
        if self.removing {
            "removing"
        } else {
            &self.status
        }
    }

    /// The tabs' names as the strip shows them: the provider, numbered from
    /// its second tab on.
    fn tab_labels(&self) -> Vec<String> {
        let providers: Vec<&str> = self.tabs.iter().map(|tab| tab.provider.as_str()).collect();
        crate::agent_tabs::tab_labels(&providers)
    }
}

/// The terminals with the projects and agents that own them, read fresh:
/// right after a change, the workspace document may not have caught up.
struct WorkspaceDocument {
    projects: Vec<Project>,
    sessions: Vec<Agent>,
    terminals: Vec<Terminal>,
}

impl WorkspaceDocument {
    fn read(client: &Client) -> Result<Self, CliError> {
        Ok(Self {
            projects: projects(client)?,
            sessions: agents(client)?,
            terminals: client.get_json("/api/v1/terminals")?,
        })
    }
}

fn projects(client: &Client) -> Result<Vec<Project>, CliError> {
    client.get_json("/api/v1/projects")
}

fn agents(client: &Client) -> Result<Vec<Agent>, CliError> {
    client.get_json("/api/v1/sessions")
}

fn find_project<'a>(projects: &'a [Project], query: &str) -> Result<&'a Project, CliError> {
    output::select("project", query, projects, |p| &p.id, |p| &p.name)
}

/// The agent `query` names, by id or by a name only it has.
fn find_agent(client: &Client, query: &str) -> Result<Agent, CliError> {
    let agents = agents(client)?;
    let names: Vec<(String, String)> = agents.iter().map(|a| (a.id.clone(), a.name())).collect();
    let found = output::select("agent", query, &names, |n| &n.0, |n| &n.1)?;
    let id = found.0.clone();
    Ok(agents.into_iter().find(|a| a.id == id).expect("selected"))
}

/// The tab of `agent` that `query` names, by id or by its strip label.
fn find_tab<'a>(agent: &'a Agent, query: &str) -> Result<(&'a Tab, String), CliError> {
    let labelled: Vec<(&Tab, String)> = agent.tabs.iter().zip(agent.tab_labels()).collect();
    let found = output::select("tab", query, &labelled, |t| &t.0.id, |t| &t.1)?;
    Ok((found.0, found.1.clone()))
}

/// `yes` for a JSON `true`, blank for anything else.
fn yes_or_blank(value: Option<&Value>) -> String {
    let flag = value.and_then(Value::as_bool).unwrap_or(false);
    if flag { "yes" } else { "" }.to_string()
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// `dux projects ls`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses.
pub fn projects_ls(client: &Client, shape: Shape) -> Result<String, CliError> {
    let agents = agents(client)?;
    let rows = projects(client)?
        .into_iter()
        .map(|project| {
            let count = agents
                .iter()
                .filter(|a| !a.removing && a.project_id() == Some(project.id.as_str()))
                .count();
            Row {
                id: project.id.clone(),
                cells: vec![
                    project.id.clone(),
                    project.name.clone(),
                    project.path.clone(),
                    project.current_branch.clone(),
                    project.branch_status.replace('_', " "),
                    count.to_string(),
                ],
                json: json!({
                    "id": project.id,
                    "name": project.name,
                    "path": project.path,
                    "branch": project.current_branch,
                    "branch_status": project.branch_status,
                    "agents": count,
                }),
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["ID", "NAME", "PATH", "BRANCH", "BRANCH STATUS", "AGENTS"],
            rows,
        },
        shape,
    ))
}

/// `dux projects show <p>`. The project's environment is named, never
/// printed: its values are often secrets.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single project.
pub fn projects_show(client: &Client, query: &str) -> Result<String, CliError> {
    let projects = projects(client)?;
    let project = find_project(&projects, query)?;
    let agents: Vec<String> = agents(client)?
        .into_iter()
        .filter(|a| a.project_id() == Some(project.id.as_str()))
        .map(|a| a.id)
        .collect();
    Ok(output::details(&json!({
        "id": project.id,
        "name": project.name,
        "path": project.path,
        "path_missing": project.path_missing,
        "branch": project.current_branch,
        "branch_status": project.branch_status,
        "leading_branch": project.leading_branch,
        "default_provider": project.default_provider,
        "startup_command": project.startup_command,
        "env": project.env.keys().collect::<Vec<_>>(),
        "agents": agents,
    })))
}

/// `dux projects worktrees ls <p>`: the worktree manager's own entries.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single project.
pub fn worktrees_ls(client: &Client, query: &str, shape: Shape) -> Result<String, CliError> {
    #[derive(Deserialize)]
    struct Entries {
        entries: Vec<Value>,
    }
    let projects = projects(client)?;
    let project = find_project(&projects, query)?;
    let names: BTreeMap<String, String> = agents(client)?
        .into_iter()
        .map(|a| (a.id.clone(), a.name()))
        .collect();
    let entries: Entries = client.get_json(&format!(
        "/api/v1/projects/{}/worktrees",
        segment(&project.id)
    ))?;
    let text = |entry: &Value, field: &str| {
        entry
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let flag = |entry: &Value, field: &str| yes_or_blank(entry.get(field));
    let rows = entries
        .entries
        .into_iter()
        .map(|entry| {
            let held_by = entry
                .get("agent_id")
                .and_then(Value::as_str)
                .map(|id| names.get(id).cloned().unwrap_or_else(|| id.to_string()))
                .unwrap_or_default();
            Row {
                id: text(&entry, "worktree_path"),
                cells: vec![
                    text(&entry, "worktree_path"),
                    text(&entry, "branch_name"),
                    flag(&entry, "dirty"),
                    held_by,
                    text(&entry, "in_use"),
                    flag(&entry, "being_removed"),
                ],
                json: entry,
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["PATH", "BRANCH", "DIRTY", "HELD BY", "IN USE", "REMOVING"],
            rows,
        },
        shape,
    ))
}

/// What `dux agents ls` lists of each agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentColumns {
    /// Every column.
    All,
    /// Only its id and the folder it works in (`--worktrees`).
    Worktree,
}

/// `dux agents ls`, of one project when `project` names one, with `columns`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `project` names no single project.
pub fn agents_ls(
    client: &Client,
    project: Option<&str>,
    columns: AgentColumns,
    shape: Shape,
) -> Result<String, CliError> {
    let projects = projects(client)?;
    let only = project
        .map(|query| find_project(&projects, query).map(|p| p.id.clone()))
        .transpose()?;
    let project_names: BTreeMap<&str, &str> = projects
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let listed = agents(client)?
        .into_iter()
        .filter(|a| only.is_none() || a.project_id() == only.as_deref());
    if columns == AgentColumns::Worktree {
        let rows = listed
            .map(|agent| Row {
                id: agent.id.clone(),
                cells: vec![agent.id.clone(), agent.directory().to_string()],
                json: json!({ "id": agent.id, "worktree_path": agent.directory() }),
            })
            .collect();
        return Ok(output::render(
            &Listing {
                headers: vec!["ID", "WORKTREE"],
                rows,
            },
            shape,
        ));
    }
    let rows = listed
        .map(|agent| {
            let project_cell = match &agent.workspace {
                Workspace::Managed { project_id, .. } => project_names
                    .get(project_id.as_str())
                    .map_or(project_id.clone(), std::string::ToString::to_string),
                Workspace::Folder { folder_label, .. } => format!("folder {folder_label}"),
            };
            let running = agent.tabs.iter().filter(|t| t.has_live_process).count();
            let viewers = agent.remote_viewers.unwrap_or(0);
            Row {
                id: agent.id.clone(),
                cells: vec![
                    agent.id.clone(),
                    agent.name(),
                    project_cell,
                    agent.directory().to_string(),
                    agent.provider.clone(),
                    agent.state().to_string(),
                    format!("{running}/{}", agent.tabs.len()),
                    viewers.to_string(),
                ],
                json: json!({
                    "id": agent.id,
                    "name": agent.name(),
                    "project": agent.project_id(),
                    "folder": match &agent.workspace {
                        Workspace::Folder { folder_path, .. } => Some(folder_path),
                        Workspace::Managed { .. } => None,
                    },
                    "worktree_path": agent.directory(),
                    "provider": agent.provider,
                    "state": agent.state(),
                    "tabs": agent.tabs.len(),
                    "running_tabs": running,
                    "remote_viewers": viewers,
                    "pull_requests": agent.prs.len(),
                    "pull_request": agent.pr.as_ref().map(Pr::shown_line),
                }),
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec![
                "ID", "NAME", "PROJECT", "WORKTREE", "PROVIDER", "STATE", "TABS", "BROWSERS",
            ],
            rows,
        },
        shape,
    ))
}

/// `dux agents show <a>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent.
///
/// # Panics
///
/// Never: the fields are built as a JSON object literal.
pub fn agents_show(client: &Client, query: &str) -> Result<String, CliError> {
    let agent = find_agent(client, query)?;
    let tabs: Vec<String> = agent
        .tabs
        .iter()
        .zip(agent.tab_labels())
        .map(|(tab, label)| {
            let running = if tab.has_live_process {
                "running"
            } else {
                "stopped"
            };
            format!("{} ({label}, {running})", tab.id)
        })
        .collect();
    let terminals: Vec<String> = agent
        .terminals
        .iter()
        .map(|t| format!("{} ({})", t.id, t.label))
        .collect();
    let mut shown = json!({
        "id": agent.id,
        "name": agent.name(),
        "provider": agent.provider,
        "state": agent.state(),
        "tabs": tabs,
        "terminals": terminals,
        "remote_viewers": agent.remote_viewers,
        "pull_request": agent.pr.as_ref().map(Pr::shown_line),
        "pull_requests": agent.prs.iter().map(Pr::list_line).collect::<Vec<_>>(),
        "pull_requests_hidden": agent.prs_hidden.iter().map(Pr::list_line).collect::<Vec<_>>(),
    });
    let fields = shown.as_object_mut().expect("an object");
    match &agent.workspace {
        Workspace::Managed {
            project_id,
            branch_name,
            worktree_path,
            ..
        } => {
            fields.insert("workspace".into(), "worktree".into());
            fields.insert("project".into(), project_id.clone().into());
            fields.insert("branch".into(), branch_name.clone().into());
            fields.insert("worktree_path".into(), worktree_path.clone().into());
        }
        Workspace::Folder { folder_path, .. } => {
            fields.insert("workspace".into(), "folder".into());
            fields.insert("folder".into(), folder_path.clone().into());
        }
    }
    Ok(output::details(&shown))
}

/// `dux agents tabs ls <a>`, in strip order.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent.
pub fn tabs_ls(client: &Client, query: &str, shape: Shape) -> Result<String, CliError> {
    let agent = find_agent(client, query)?;
    let rows = agent
        .tabs
        .iter()
        .zip(agent.tab_labels())
        .map(|(tab, label)| {
            let state = if tab.needs_attention {
                "needs you"
            } else if tab.working {
                "working"
            } else if tab.has_live_process {
                "running"
            } else {
                "stopped"
            };
            Row {
                id: tab.id.clone(),
                cells: vec![
                    tab.id.clone(),
                    label.clone(),
                    tab.provider.clone(),
                    state.to_string(),
                ],
                json: json!({
                    "id": tab.id,
                    "label": label,
                    "provider": tab.provider,
                    "running": tab.has_live_process,
                    "state": state,
                }),
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["ID", "LABEL", "PROVIDER", "STATE"],
            rows,
        },
        shape,
    ))
}

/// The pull request of `agent` that `query` names, read the way an attach
/// reads it: its URL (a link copied from any of its pages, host and
/// repository without regard to case), `owner/repo#n` (on any host), or `#n`
/// / `n` (the number in any repository). Hidden ones count.
fn find_pr<'a>(agent: &'a Agent, query: &str) -> Result<&'a Pr, CliError> {
    let every = agent.prs.iter().chain(&agent.prs_hidden);
    let named = crate::pr_reference::parse_typed_reference(query).ok();
    let repository = named.as_ref().and_then(|named| named.owner_repo.as_deref());
    let shown = match (&named, repository) {
        (
            Some(TypedReference {
                number: Some(number),
                ..
            }),
            None,
        ) => format!("#{number}"),
        _ => query.trim().to_string(),
    };
    let matches: Vec<&Pr> = match &named {
        Some(
            reference @ TypedReference {
                number: Some(number),
                ..
            },
        ) => every
            .filter(|pr| {
                pr.number == *number
                    && (repository.is_none() || reference.matches(&pr.host, &pr.owner_repo))
            })
            .collect(),
        _ => Vec::new(),
    };
    let agent_name = agent.name();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(CliError::new(
            Exit::Failed,
            format!(
                "agent {agent_name} has no pull request {shown}; dux agents prs ls {agent_name} \
                 lists them"
            ),
        )),
        many => {
            let urls: Vec<&str> = many.iter().map(|pr| pr.url.as_str()).collect();
            let how = match repository {
                Some(_) => "name one by its URL".to_string(),
                None => format!("name one as owner/repo{shown} or by its URL"),
            };
            Err(CliError::new(
                Exit::Usage,
                format!(
                    "{} pull requests on agent {agent_name} are {shown}: {}; {how}",
                    many.len(),
                    urls.join(", ")
                ),
            ))
        }
    }
}

/// `dux agents prs ls <a>`: the shown ones in the agent's order, the one its
/// banner shows first, then the hidden ones.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent or a
/// standalone one.
pub fn prs_ls(client: &Client, query: &str, shape: Shape) -> Result<String, CliError> {
    let agent = pr_agent(client, query)?;
    let positions = (1..).map(Some).take(agent.prs.len());
    let rows = agent
        .prs
        .iter()
        .zip(positions)
        .chain(agent.prs_hidden.iter().map(|pr| (pr, None)))
        .map(|(pr, position): (&Pr, Option<usize>)| Row {
            id: pr.url.clone(),
            cells: vec![
                position.map_or_else(|| "hidden".to_string(), |n| n.to_string()),
                pr.label(),
                pr.state.clone(),
                pr.branch().unwrap_or_default().to_string(),
                pr.source().to_string(),
                pr.url.clone(),
                pr.title.clone(),
            ],
            json: json!({
                "position": position,
                "shown": position == Some(1),
                "hidden": position.is_none(),
                "host": pr.host,
                "owner_repo": pr.owner_repo,
                "number": pr.number,
                "state": pr.state,
                "head_branch": pr.head_branch,
                "found": pr.found,
                "attached": pr.attached,
                "url": pr.url,
                "title": pr.title,
                "created_at": pr.created_at,
                "updated_at": pr.updated_at,
            }),
        })
        .collect();
    let mut out = output::render(
        &Listing {
            headers: vec!["POS", "PR", "STATE", "BRANCH", "SOURCE", "URL", "TITLE"],
            rows,
        },
        shape,
    );
    if shape == Shape::Table && agent.pr_sort == "manual" {
        let _ = writeln!(
            out,
            "In your own order (dux agents prs sort {} auto goes back).",
            agent.name()
        );
    }
    Ok(out)
}

/// `dux agents prs branches <a>`: every branch the agent's worktree has been
/// on, the one checked out now first and the rest newest first, with its pull
/// requests, then the attached pull requests
/// that are on none of them.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent or a
/// standalone one.
pub fn prs_branches(client: &Client, query: &str, shape: Shape) -> Result<String, CliError> {
    let agent = pr_agent(client, query)?;
    let answer: BranchesAnswer = client.get_json(&format!(
        "/api/v1/sessions/{}/pull-requests/branches",
        segment(&agent.id)
    ))?;
    let named = |keys: &[PrKeyAnswer]| -> (String, Vec<Value>) {
        let mut words = Vec::new();
        let mut objects = Vec::new();
        for key in keys {
            let shown = agent.prs.iter().find(|pr| pr.is(key));
            let hidden = agent.prs_hidden.iter().find(|pr| pr.is(key));
            let Some(pr) = shown.or(hidden) else {
                words.push(format!("{}#{}", key.owner_repo, key.number));
                objects.push(json!({
                    "host": key.host,
                    "owner_repo": key.owner_repo,
                    "number": key.number,
                    "state": null,
                    "hidden": null,
                    "url": null,
                }));
                continue;
            };
            let suffix = if hidden.is_some() { " (hidden)" } else { "" };
            words.push(format!("{} {}{suffix}", pr.label(), pr.state));
            objects.push(json!({
                "host": pr.host,
                "owner_repo": pr.owner_repo,
                "number": pr.number,
                "state": pr.state,
                "hidden": hidden.is_some(),
                "url": pr.url,
            }));
        }
        (words.join(", "), objects)
    };
    let mut rows: Vec<Row> = answer
        .branches
        .iter()
        .map(|branch| {
            let (words, objects) = named(&branch.prs);
            Row {
                id: branch.name.clone(),
                cells: vec![
                    branch.name.clone(),
                    if branch.current { "yes" } else { "" }.to_string(),
                    branch.first_seen_at.clone().unwrap_or_default(),
                    branch.former_names.join(", "),
                    words,
                ],
                json: json!({
                    "branch": branch.name,
                    "current": branch.current,
                    "first_seen_at": branch.first_seen_at,
                    "former_names": branch.former_names,
                    "pull_requests": objects,
                }),
            }
        })
        .collect();
    // The attachments on no branch have no branch to be the id of, so a list
    // of ids leaves them out.
    if !answer.attached_on_no_branch.is_empty() && shape != Shape::Ids {
        let (words, objects) = named(&answer.attached_on_no_branch);
        rows.push(Row {
            id: String::new(),
            cells: vec![
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                words,
            ],
            json: json!({
                "branch": null,
                "current": false,
                "first_seen_at": null,
                "former_names": [],
                "pull_requests": objects,
            }),
        });
    }
    Ok(output::render(
        &Listing {
            headers: vec!["BRANCH", "CURRENT", "FIRST SEEN", "FORMER NAMES", "PRS"],
            rows,
        },
        shape,
    ))
}

/// What a terminal belongs to, in words.
fn owner_words(owner: &Owner, document: &WorkspaceDocument) -> String {
    match owner {
        Owner::Session { session_id } => {
            let name = document
                .sessions
                .iter()
                .find(|a| &a.id == session_id)
                .map_or(session_id.clone(), Agent::name);
            format!("agent {name}")
        }
        Owner::Project { project_id } => {
            let name = document
                .projects
                .iter()
                .find(|p| &p.id == project_id)
                .map_or(project_id.clone(), |p| p.name.clone());
            format!("project {name}")
        }
        Owner::Standalone { cwd_label } => format!("standalone in {cwd_label}"),
    }
}

/// `dux terminals ls`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses.
pub fn terminals_ls(client: &Client, shape: Shape) -> Result<String, CliError> {
    let document = WorkspaceDocument::read(client)?;
    let rows = document
        .terminals
        .iter()
        .map(|terminal| {
            let owner = owner_words(&terminal.owner, &document);
            let running = terminal.foreground_cmd.clone().unwrap_or_default();
            let (kind, owner_id) = match &terminal.owner {
                Owner::Session { session_id } => ("agent", Some(session_id)),
                Owner::Project { project_id } => ("project", Some(project_id)),
                Owner::Standalone { .. } => ("standalone", None),
            };
            Row {
                id: terminal.id.clone(),
                cells: vec![
                    terminal.id.clone(),
                    terminal.label.clone(),
                    owner.clone(),
                    running.clone(),
                ],
                json: json!({
                    "id": terminal.id,
                    "label": terminal.label,
                    "owner": kind,
                    "owner_id": owner_id,
                    "owner_name": owner,
                    "running": terminal.foreground_cmd,
                }),
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["ID", "LABEL", "OWNER", "RUNNING"],
            rows,
        },
        shape,
    ))
}

// ---------------------------------------------------------------------------
// Changes
// ---------------------------------------------------------------------------

/// What a change does when somebody else is attached to what it would end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfAttached {
    /// Refuse, naming who.
    Refuse,
    /// Go ahead anyway (`--dangerously-ignore-connected`).
    GoAhead,
}

/// Whether a removal deletes the worktrees too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Worktrees {
    /// They stay on disk.
    Keep,
    /// They are deleted.
    Delete,
}

impl Worktrees {
    fn deleted(self) -> bool {
        self == Worktrees::Delete
    }
}

/// Whether a create agrees to a review copy of a pull request whose own
/// branch cannot be used as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewCopy {
    /// Not agreed: a create that needs one is refused.
    NotAgreed,
    /// Agreed (`--review-copy`).
    Agreed,
}

impl ReviewCopy {
    fn agreed(self) -> bool {
        self == ReviewCopy::Agreed
    }
}

/// The choices `dux projects add` passes along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectAddChoices {
    /// Check out the default branch in the folder first.
    pub checkout_default: bool,
    /// Make the folder a git repository when it is not one.
    pub init_repo: bool,
}

/// A change worked out and not sent yet: what to ask before it, and the
/// request that makes it.
#[derive(Debug)]
pub struct Planned {
    /// The question asked before it goes ahead, naming what it changes.
    pub question: String,
    /// A sentence printed before the question, for what the answer is agreeing
    /// to beyond what the question says.
    pub notice: Option<String>,
    method: Method,
    path: String,
    body: Option<Value>,
}

impl Planned {
    fn new(question: String, method: Method, path: String, body: Option<Value>) -> Self {
        Self {
            question,
            notice: None,
            method,
            path,
            body,
        }
    }

    /// Go ahead even though somebody else is attached to what this ends,
    /// when `if_attached` says so.
    fn forced(mut self, if_attached: IfAttached) -> Self {
        if if_attached == IfAttached::GoAhead {
            let separator = if self.path.contains('?') { '&' } else { '?' };
            self.path.push(separator);
            self.path.push_str("force_connected=true");
        }
        self
    }
}

/// How often a create refused for another create that named no operation is sent again.
const CREATE_RETRY_PAUSE: Duration = Duration::from_millis(500);

/// Send a planned change and, unless `wait` is `None`, wait for its outcome. A finished change
/// prints its sentence, each part, then the ids it created, last; with no wait, the operation's id.
///
/// A create refused only because another create is running waits for that one to
/// finish, sends once more, and says so on stderr. That waiting and the new create's own
/// wait share the one `wait`, and no create is started once it has run out. With no wait the
/// create is refused as any other change is.
///
/// # Errors
///
/// Fails when the dux cannot be reached, refuses the change, or the change fails, does only part of
/// its work, or is still running when `wait` runs out.
pub fn perform(
    client: &Client,
    planned: &Planned,
    wait: Option<Duration>,
) -> Result<String, CliError> {
    let deadline = wait.map(wait::deadline_after);
    let record = match client.try_change(planned.method, &planned.path, planned.body.clone(), None)
    {
        Ok(record) => record,
        Err(refused) => match deadline {
            Some(deadline) if refused.error.message == CREATE_IN_FLIGHT_REFUSAL => {
                retry_after_other_create(client, planned, refused, deadline)?
            }
            _ => return Err(refused.error),
        },
    };
    let Some(deadline) = deadline else {
        return Ok(format!("{}\n", record.id));
    };
    let record = client.wait(record, deadline.saturating_duration_since(Instant::now()))?;
    report(&record)
}

/// Wait for the create `refused` named to finish, or, when it named none, send the create
/// again every [`CREATE_RETRY_PAUSE`]; then, with `deadline` still ahead, send it again.
fn retry_after_other_create(
    client: &Client,
    planned: &Planned,
    refused: wait::Refused,
    deadline: Instant,
) -> Result<OperationRecord, CliError> {
    let send = || {
        let left = deadline.saturating_duration_since(Instant::now());
        client.try_change(
            planned.method,
            &planned.path,
            planned.body.clone(),
            Some(left),
        )
    };
    let Some(other) = refused.operation.as_deref() else {
        let _ = writeln!(
            std::io::stderr().lock(),
            "Another agent was being created, so this one waited for it."
        );
        let mut last = refused.error;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            std::thread::sleep(CREATE_RETRY_PAUSE.min(left));
            if Instant::now() >= deadline {
                return Err(last);
            }
            match send() {
                Ok(record) => return Ok(record),
                Err(again) if again.error.message == CREATE_IN_FLIGHT_REFUSAL => {
                    last = again.error;
                }
                Err(again) => return Err(again.error),
            }
        }
    };
    let _ = writeln!(
        std::io::stderr().lock(),
        "Another agent was being created (operation {other}), so this one waited for it."
    );
    // Time running out on somebody else's creation leaves this one never started:
    // refused, not of unknown outcome.
    match client.wait_for_other(other, deadline) {
        Err(error) if error.exit == Exit::Unknown => return Err(refused.error),
        other => other?,
    }
    if Instant::now() >= deadline {
        return Err(refused.error);
    }
    send().map_err(|again| again.error)
}

fn report(record: &OperationRecord) -> Result<String, CliError> {
    if record.state != RecordState::Succeeded {
        return wait::outcome(record);
    }
    let mut lines = Vec::new();
    if !record.message.is_empty() {
        lines.push(record.message.clone());
    }
    lines.extend(record.parts.iter().map(wait::part_line));
    lines.extend(record.created.iter().cloned());
    Ok(lines
        .iter()
        .flat_map(|line| [line.as_str(), "\n"])
        .collect())
}

/// `dux projects add <path>`.
///
/// # Errors
///
/// Fails when `path` cannot be given to the target dux.
pub fn projects_add(
    client: &Client,
    path: &str,
    name: Option<&str>,
    choices: ProjectAddChoices,
    cwd: &Path,
) -> Result<Planned, CliError> {
    let path = output::path_for_target(path, client.remote_name(), cwd)?;
    Ok(Planned::new(
        format!("Add the project at {path}"),
        Method::Post,
        "/api/v1/projects".to_string(),
        Some(json!({
            "path": path,
            "name": name.unwrap_or_default(),
            "checkout_default": choices.checkout_default,
            "init_repo": choices.init_repo,
        })),
    ))
}

/// `dux projects rm <p>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single project.
pub fn projects_rm(
    client: &Client,
    query: &str,
    worktrees: Worktrees,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let projects = projects(client)?;
    let project = find_project(&projects, query)?;
    let delete_worktrees = worktrees.deleted();
    let question = if delete_worktrees {
        format!(
            "Remove project {} and delete its agents and their worktrees",
            project.name
        )
    } else {
        format!(
            "Remove project {} (its agents' worktrees stay on disk)",
            project.name
        )
    };
    Ok(Planned::new(
        question,
        Method::Delete,
        format!(
            "/api/v1/projects/{}?delete_worktrees={delete_worktrees}",
            segment(&project.id)
        ),
        None,
    )
    .forced(if_attached))
}

/// How `dux agents add` makes its agent.
#[derive(Debug)]
pub enum NewAgent {
    /// A new branch and worktree in a project.
    Branch {
        /// The project, by id or name.
        project: String,
        /// Whether to attach to a branch of that name that already exists.
        existing_branch: bool,
        /// Whether the project checkout's uncommitted changes are copied into the new worktree.
        copy_uncommitted: bool,
    },
    /// From a pull request of a project. `review_copy` agrees to a review copy
    /// of it when the pull request's own branch cannot be used as it is.
    PullRequest {
        /// The project, by id or name.
        project: String,
        /// The pull request reference as typed.
        reference: String,
        /// Whether a review copy is agreed to.
        review_copy: ReviewCopy,
    },
    /// From a pull request named by its address or `owner/repo#123`, in
    /// whichever project has its repository, or in a new one cloned (or added
    /// from the existing checkout at `path`) when none does.
    PullRequestAnywhere {
        /// The pull request's address or `owner/repo#123`.
        reference: String,
        /// A folder holding a checkout of the repository, or `None` to clone one.
        path: Option<String>,
        /// Whether a review copy is agreed to.
        review_copy: ReviewCopy,
    },
    /// From a worktree a project already has.
    Worktree {
        /// The project, by id or name.
        project: String,
        /// The worktree's path.
        path: String,
    },
    /// A copy of another agent's worktree.
    Fork {
        /// The agent whose worktree is copied, by id or name.
        agent: String,
    },
    /// In a folder of the user's own, with no project.
    Standalone {
        /// The folder to run in.
        folder: String,
        /// The provider to launch, or `None` for the global default.
        provider: Option<String>,
    },
}

/// `dux agents add`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, a project or agent named names no single one,
/// or a path cannot be given to the target dux.
pub fn agents_add(
    client: &Client,
    how: NewAgent,
    name: Option<&str>,
    cwd: &Path,
) -> Result<Planned, CliError> {
    let name = name.unwrap_or_default();
    let called = if name.is_empty() {
        "an agent".to_string()
    } else {
        format!("agent {name}")
    };
    let project_id = |query: &str| -> Result<(String, String), CliError> {
        let projects = projects(client)?;
        let project = find_project(&projects, query)?;
        Ok((project.id.clone(), project.name.clone()))
    };
    let (question, body) = match how {
        NewAgent::Branch {
            project,
            existing_branch,
            copy_uncommitted,
        } => {
            let (id, project) = project_id(&project)?;
            (
                format!("Create {called} in project {project}"),
                json!({
                    "kind": "new",
                    "project_id": id,
                    "name": name,
                    "use_existing_branch": existing_branch,
                    "copy_uncommitted_changes": copy_uncommitted,
                }),
            )
        }
        NewAgent::PullRequest {
            project,
            reference,
            review_copy,
        } => {
            let (id, project) = project_id(&project)?;
            pull_request_in_project(&id, &project, &called, &reference, name, review_copy)
        }
        NewAgent::PullRequestAnywhere {
            reference,
            path,
            review_copy,
        } => {
            return pull_request_anywhere(
                client,
                &reference,
                path.as_deref(),
                review_copy,
                name,
                &called,
                cwd,
            );
        }
        NewAgent::Worktree { project, path } => {
            let (id, project) = project_id(&project)?;
            let path = output::path_for_target(&path, client.remote_name(), cwd)?;
            (
                format!("Create {called} on the worktree {path} of project {project}"),
                json!({
                    "kind": "from_worktree",
                    "project_id": id,
                    "worktree_path": path,
                    "name": name,
                }),
            )
        }
        NewAgent::Fork { agent } => {
            let source = find_agent(client, &agent)?;
            (
                format!("Fork agent {} into {called}", source.name()),
                json!({ "kind": "fork", "session_id": source.id, "name": name }),
            )
        }
        NewAgent::Standalone { folder, provider } => {
            let folder = output::path_for_target(&folder, client.remote_name(), cwd)?;
            (
                format!("Create {called} in the folder {folder}"),
                json!({
                    "kind": "standalone",
                    "folder": folder,
                    "name": name,
                    "provider": provider,
                }),
            )
        }
    };
    Ok(Planned::new(
        question,
        Method::Post,
        "/api/v1/sessions".to_string(),
        Some(body),
    ))
}

/// The question and request for an agent from a pull request of a project
/// dux already has.
fn pull_request_in_project(
    id: &str,
    project: &str,
    called: &str,
    reference: &str,
    name: &str,
    review_copy: ReviewCopy,
) -> (String, Value) {
    (
        format!("Create {called} from pull request {reference} in project {project}"),
        json!({
            "kind": "from_pr",
            "project_id": id,
            "pr": reference,
            "name": name,
            "review_copy": review_copy.agreed(),
            // This client agrees with a flag, so a refusal names it.
            "review_copy_by_flag": true,
        }),
    )
}

/// A pull request number on its own, which names no repository.
fn is_bare_number(reference: &str) -> bool {
    let digits = reference.trim().trim_start_matches('#');
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

#[derive(Deserialize)]
struct ResolvedReference {
    repository: Option<String>,
    #[serde(default)]
    projects: Vec<ResolvedProject>,
    #[serde(default)]
    uninspected_summary: Option<String>,
    #[serde(default)]
    clone: Option<CloneOffer>,
    #[serde(default)]
    clone_unavailable: Option<String>,
}

#[derive(Deserialize)]
struct ResolvedProject {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct CloneOffer {
    address: String,
    host: String,
    owner_repo: String,
    number: u64,
    head_branch: String,
    #[serde(default)]
    head_oid: Option<String>,
    review_copy: ReviewCopyOutlook,
}

#[derive(Deserialize)]
struct FolderCheck {
    state: String,
    path: String,
    #[serde(default)]
    review_copy: Option<ReviewCopyOutlook>,
}

/// POST `body` to `path` and read the 200 answer. A 400 is the command's
/// refusal with the server's sentence; anything else is the usual error.
fn post_for_answer<T: serde::de::DeserializeOwned>(
    client: &Client,
    path: &str,
    body: &Value,
) -> Result<T, CliError> {
    let reply = client.send(super::transport::Request {
        method: Method::Post,
        path: path.to_string(),
        body: Some(body.to_string().into_bytes()),
        bearer: None,
        timeout: None,
    })?;
    match reply.status {
        200 => client.parse(&reply),
        400 => Err(CliError::new(
            Exit::Refused,
            super::connect::reply_sentence(&reply),
        )),
        _ => Err(client.refusal(&reply)),
    }
}

/// `dux agents add --from-pr <reference>` with no project: the project that
/// has the repository, or the clone (or the add of an existing checkout) that
/// makes one.
fn pull_request_anywhere(
    client: &Client,
    reference: &str,
    path: Option<&str>,
    review_copy: ReviewCopy,
    name: &str,
    called: &str,
    cwd: &Path,
) -> Result<Planned, CliError> {
    if is_bare_number(reference) {
        return Err(CliError::new(
            Exit::Refused,
            "A pull request number on its own does not say which repository it is in. Pass a \
             link or owner/repo#123, or add --project to choose one.",
        ));
    }
    let resolved: ResolvedReference = post_for_answer(
        client,
        "/api/v1/pull-requests/resolve",
        &json!({ "reference": reference }),
    )?;
    let repository = resolved
        .repository
        .clone()
        .unwrap_or_else(|| reference.to_string());
    match resolved.projects.as_slice() {
        [project] => {
            return plan_in_project(
                client,
                project,
                &repository,
                ProjectAsk {
                    reference,
                    path,
                    review_copy,
                    name,
                    called,
                    cwd,
                },
            );
        }
        [] => {}
        several => {
            let names: Vec<&str> = several.iter().map(|p| p.name.as_str()).collect();
            return Err(CliError::new(
                Exit::Refused,
                format!(
                    "{repository} is checked out in more than one project ({}); add --project \
                     to choose one.",
                    names.join(", ")
                ),
            ));
        }
    }
    let Some(offer) = resolved.clone else {
        return Err(no_project_refusal(
            resolved.uninspected_summary.as_deref(),
            resolved.clone_unavailable.as_deref(),
            &repository,
        ));
    };

    let checked = check_clone_folder(client, &offer, &clone_folder(client, &offer, path, cwd)?)?;
    let adding = checked.state == "checkout";

    let typed = name.trim();
    let refusal_ending = if adding {
        "Nothing was added."
    } else {
        "Nothing was cloned."
    };
    let outlook = clone_review_copy_outlook(&offer, &checked, typed);
    let mut notices = Vec::new();
    if !adding && let Some(summary) = &resolved.uninspected_summary {
        notices.push(format!(
            "dux could not find a checked project for {repository}. dux could not check every \
             project ({summary}), so one of them may already be a checkout of it. dux will \
             clone it, add it as a project, and create the agent from pull request #{}.",
            offer.number
        ));
    }
    refuse_or_note_review_copy(outlook, review_copy, refusal_ending, &mut notices)?;
    let notice = (!notices.is_empty()).then(|| notices.join("\n"));

    let question = clone_question(&offer, &checked, called, reference);
    let mut body = json!({
        "path": checked.path,
        "pull_request": {
            "host": offer.host,
            "owner_repo": offer.owner_repo,
            "number": offer.number,
        },
        "add_existing": adding,
        "review_copy": review_copy.agreed(),
        // This client agrees with a flag, so a refusal names it.
        "review_copy_by_flag": !review_copy.agreed(),
    });
    if !typed.is_empty() {
        body["agent_name"] = json!(typed);
    }
    let mut planned = Planned::new(
        question,
        Method::Post,
        "/api/v1/projects".to_string(),
        Some(body),
    );
    planned.notice = notice;
    Ok(planned)
}

/// What the server makes of `folder` as the destination of a clone for
/// `offer`: free, or already a checkout of the repository.
///
/// # Errors
///
/// Fails when the server refuses the folder or cannot be asked.
fn check_clone_folder(
    client: &Client,
    offer: &CloneOffer,
    folder: &str,
) -> Result<FolderCheck, CliError> {
    let mut check = json!({
        "path": folder,
        "host": offer.host,
        "owner_repo": offer.owner_repo,
        "number": offer.number,
        "head_branch": offer.head_branch,
    });
    if let Some(oid) = &offer.head_oid {
        check["head_oid"] = json!(oid);
    }
    post_for_answer(client, "/api/v1/pull-requests/clone-destination", &check)
}

/// Whether the agent named `typed` (the pull request's branch when empty) for
/// `offer` needs a review copy in the folder as `checked`. A checkout's answer
/// is worked out against that folder and replaces the offer's; both describe
/// the agent only when it takes the branch's name. A name spelled like a copy
/// of the branch is one whatever the branch holds. An add cannot tell whether
/// a branch of that name already exists, which makes a copy unnecessary, so
/// the server asks for that one.
fn clone_review_copy_outlook(
    offer: &CloneOffer,
    checked: &FolderCheck,
    typed: &str,
) -> ReviewCopyOutlook {
    let adding = checked.state == "checkout";
    if typed.is_empty() || typed == offer.head_branch {
        if adding {
            checked
                .review_copy
                .clone()
                .unwrap_or(ReviewCopyOutlook::Unknown)
        } else {
            offer.review_copy.clone()
        }
    } else if adding {
        ReviewCopyOutlook::NotNeeded
    } else {
        crate::agent_job::typed_copy_review_outlook(&offer.head_branch, typed, offer.number)
            .unwrap_or(ReviewCopyOutlook::NotNeeded)
    }
}

/// The question a clone (or an add of the checkout `checked` found) for
/// `offer` asks, creating the agent `called` from the pull request
/// `reference`.
fn clone_question(
    offer: &CloneOffer,
    checked: &FolderCheck,
    called: &str,
    reference: &str,
) -> String {
    if checked.state == "checkout" {
        format!(
            "Add the folder {} as a project and create {called} from pull request {reference}",
            checked.path
        )
    } else {
        format!(
            "Clone {} into {} and create {called} from pull request {reference}",
            offer.address, checked.path
        )
    }
}

/// What `dux agents add --from-pr` asked for, beside where it goes.
#[derive(Clone, Copy)]
struct ProjectAsk<'a> {
    /// The pull request, as typed.
    reference: &'a str,
    /// The folder `--path` names.
    path: Option<&'a str>,
    /// Whether a review copy is agreed to.
    review_copy: ReviewCopy,
    /// The agent's name, as typed.
    name: &'a str,
    /// How the agent is called in the question.
    called: &'a str,
    /// The directory the command runs in.
    cwd: &'a Path,
}

/// [`pull_request_anywhere`] for the one `project` that has `repository`: the
/// agent is created there, and a `--path` to any other folder is refused.
///
/// # Errors
///
/// Fails when `--path` names a folder other than the project's own.
fn plan_in_project(
    client: &Client,
    project: &ResolvedProject,
    repository: &str,
    ask: ProjectAsk<'_>,
) -> Result<Planned, CliError> {
    let ProjectAsk {
        reference,
        path,
        review_copy,
        name,
        called,
        cwd,
    } = ask;
    // Running the command again with the folder the first run used is
    // how a review copy asked about afterwards is agreed to, so the
    // project's own folder is not a different folder.
    let own_folder = path.is_some_and(|path| {
        let Ok(typed) = output::path_for_target(path, client.remote_name(), cwd) else {
            return false;
        };
        projects(client).is_ok_and(|all| {
            all.iter().any(|p| {
                p.id == project.id && crate::project_browser::same_directory(&p.path, &typed)
            })
        })
    });
    if let Some(path) = path.filter(|_| !own_folder) {
        return Err(CliError::new(
            Exit::Refused,
            format!(
                "{repository} is already a checkout in project {}, so there is nothing \
                 to clone or add at {path}. Drop --path to create the agent in that \
                 project.",
                project.name
            ),
        ));
    }
    let (question, body) = pull_request_in_project(
        &project.id,
        &project.name,
        called,
        reference,
        name,
        review_copy,
    );
    Ok(Planned::new(
        question,
        Method::Post,
        "/api/v1/sessions".to_string(),
        Some(body),
    ))
}

/// The refusal when no project has `repository` and the server offers no clone:
/// why dux could not check every project (`uninspected_summary`), and why it
/// could not offer a clone (`clone_unavailable`).
fn no_project_refusal(
    uninspected_summary: Option<&str>,
    clone_unavailable: Option<&str>,
    repository: &str,
) -> CliError {
    let mut sentence = match uninspected_summary {
        Some(summary) => format!(
            "No project dux could check is a checkout of {repository}, and dux could not \
             check every project ({summary})."
        ),
        None => format!("No project in dux is a checkout of {repository}."),
    };
    if let Some(why) = clone_unavailable {
        let why = why.trim_end();
        let stop = if why.ends_with(['.', '!', '?']) {
            ""
        } else {
            "."
        };
        let _ = write!(
            sentence,
            " dux could not look the pull request up to offer a clone: {why}{stop}"
        );
    }
    sentence.push_str(" Add --project to choose one.");
    CliError::new(Exit::Refused, sentence)
}

/// The folder a clone for `offer` goes into: the one `path` names, or one
/// named after the repository in the folder the server browses from.
///
/// # Errors
///
/// Fails when `path` cannot be resolved for the target, or the server cannot
/// be asked.
fn clone_folder(
    client: &Client,
    offer: &CloneOffer,
    path: Option<&str>,
    cwd: &Path,
) -> Result<String, CliError> {
    Ok(if let Some(path) = path {
        output::path_for_target(path, client.remote_name(), cwd)?
    } else {
        #[derive(Deserialize)]
        struct Browse {
            path: String,
        }
        let start: Browse = client.get_json("/api/v1/browse")?;
        // The offer's address is one dux built, exactly `owner/repo` on its
        // host, so no host reading can cut it and `github.com` alone will do.
        let dir = crate::clone_project::clone_dir_name(
            &offer.address,
            &crate::gh::GithubHosts::github_com_only(),
        )
        .unwrap_or_else(|| {
            offer
                .owner_repo
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string()
        });
        format!("{}/{dir}", start.path.trim_end_matches('/'))
    })
}

/// Refuse a pull request whose agent needs a review copy that `review_copy`
/// does not agree to, or that no free name can be given to; an agreed copy's
/// warning joins `notices`. `refusal_ending` ends each refusal.
///
/// # Errors
///
/// Fails in either refused case.
fn refuse_or_note_review_copy(
    outlook: ReviewCopyOutlook,
    review_copy: ReviewCopy,
    refusal_ending: &str,
    notices: &mut Vec<String>,
) -> Result<(), CliError> {
    match outlook {
        ReviewCopyOutlook::NoFreeName { message, .. } => {
            return Err(CliError::new(
                Exit::Refused,
                format!("{} {refusal_ending}", Prose::from_segments(message).plain()),
            ));
        }
        ReviewCopyOutlook::Needed { warning, .. } => {
            let warning = Prose::from_segments(warning).plain();
            if !review_copy.agreed() {
                return Err(CliError::new(
                    Exit::Refused,
                    format!(
                        "{warning}{} {refusal_ending}",
                        crate::agent_job::review_copy_how(
                            crate::agent_job::AgreementChannel::CommandLineFlag
                        )
                    ),
                ));
            }
            notices.push(warning);
        }
        ReviewCopyOutlook::NotNeeded | ReviewCopyOutlook::Unknown => {}
    }
    Ok(())
}

/// A create refused because the branch it would make already exists answers
/// with that branch, not a sentence; this is the sentence.
pub fn existing_branch_refusal(error: CliError) -> CliError {
    #[derive(Deserialize)]
    struct Existing {
        name: String,
        location: String,
    }
    #[derive(Deserialize)]
    struct Refusal {
        existing_branch: Existing,
    }
    match serde_json::from_str::<Refusal>(&error.message) {
        Ok(Refusal { existing_branch }) => CliError::new(
            Exit::Refused,
            format!(
                "A branch named {} already exists ({}); add --existing-branch to create the \
                 agent on it, or pick another --name",
                existing_branch.name, existing_branch.location
            ),
        ),
        Err(_) => error,
    }
}

/// What `branch-unpushed` answers: the branches a delete would offer, each
/// pinned to the commit it points at, and what ticking each box loses.
#[derive(Deserialize)]
struct BranchOfferReply {
    #[serde(default)]
    current_name: String,
    current: Option<crate::branch_history::OfferedBranch>,
    #[serde(default)]
    current_unpushed: Option<crate::git::UnpushedCommits>,
    #[serde(default)]
    current_unpushed_error: Option<String>,
    others: Vec<crate::branch_history::OfferedBranch>,
    #[serde(default)]
    others_unpushed: Option<crate::git::UnpushedCommits>,
    #[serde(default)]
    others_unpushed_error: Option<String>,
    #[serde(default)]
    both_unpushed: Option<crate::git::UnpushedCommits>,
    #[serde(default)]
    both_unpushed_error: Option<String>,
    #[serde(default)]
    protected: Vec<String>,
}

/// A count as the server answered it, or why it could not count.
fn count_answer(
    count: Option<crate::git::UnpushedCommits>,
    error: Option<&String>,
) -> crate::delete_agent_prose::UnpushedAnswer {
    count.ok_or_else(|| {
        error
            .cloned()
            .unwrap_or_else(|| "no count came back".to_string())
    })
}

impl BranchOfferReply {
    /// The first box's default, as the dialogs show it.
    fn current_default(&self) -> bool {
        self.current.as_ref().is_some_and(|current| {
            matches!(
                current.origin,
                crate::branch_history::BranchOrigin::Dux
                    | crate::branch_history::BranchOrigin::After
            )
        })
    }

    /// The second box's default, as the dialogs show it.
    fn others_default(&self) -> bool {
        !self.others.is_empty() && self.others.iter().all(|other| other.origin.is_duxs())
    }
}

/// One box over `branches` with its answer: a ticked box pins every branch
/// to the commit the offer read, and either way it says what it showed.
fn box_over(
    branches: &[&crate::branch_history::OfferedBranch],
    state: crate::delete_agent_prose::BoxState,
) -> crate::branch_history::BoxWire {
    let delete = state == crate::delete_agent_prose::BoxState::Ticked;
    crate::branch_history::BoxWire {
        delete,
        branches: if delete {
            branches
                .iter()
                .map(|branch| crate::branch_history::PinnedBranch {
                    name: branch.name.clone(),
                    oid: branch.oid.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        unchecked: None,
        offered: branches.iter().map(|branch| branch.name.clone()).collect(),
    }
}

fn not_offered() -> crate::branch_history::BoxWire {
    crate::branch_history::BoxWire {
        unchecked: Some(crate::branch_history::Unchecked::NotOffered),
        ..crate::branch_history::BoxWire::default()
    }
}

/// Both boxes' answers for a delete that deletes a branch, exactly as the
/// dialogs would send them: each box the flags named, ticked or not as they
/// say, and each box they did not name at the default the dialogs show. The
/// question gains what becomes of every branch, and the notice what deleting
/// them loses.
struct BranchAnswersPlan {
    answers: crate::branch_history::BranchAnswersWire,
    question: String,
    notice: Option<String>,
}

/// What deleting the branches the two answers tick loses, as the line before
/// the question, using the count for exactly that tick combination; `None`
/// when they tick nothing or deleting them loses no commit.
fn loss_note(
    offer: &BranchOfferReply,
    current: &crate::branch_history::BoxWire,
    others: &crate::branch_history::BoxWire,
) -> Option<String> {
    let mut deleted: Vec<&str> = Vec::new();
    if current.delete {
        deleted.extend(offer.current.iter().map(|current| current.name.as_str()));
    }
    if others.delete {
        deleted.extend(offer.others.iter().map(|other| other.name.as_str()));
    }
    let lost = match (current.delete, others.delete) {
        (true, true) => count_answer(offer.both_unpushed, offer.both_unpushed_error.as_ref()),
        (true, false) => count_answer(
            offer.current_unpushed,
            offer.current_unpushed_error.as_ref(),
        ),
        (false, true) => count_answer(offer.others_unpushed, offer.others_unpushed_error.as_ref()),
        (false, false) => return None,
    };
    crate::delete_agent_prose::branch_loss_prose(&deleted, &lost).map(|prose| prose.plain())
}

fn plan_branch_answers(
    offer: &BranchOfferReply,
    delete_branch: Option<bool>,
    delete_other_branches: Option<bool>,
    agent: &str,
) -> Result<BranchAnswersPlan, CliError> {
    use crate::delete_agent_prose::BoxState;
    let state = |ticked: bool| {
        if ticked {
            BoxState::Ticked
        } else {
            BoxState::Unticked
        }
    };
    let mut question = String::new();
    let mut notes: Vec<String> = Vec::new();
    let current_ticked = delete_branch.unwrap_or_else(|| offer.current_default());
    let current = match &offer.current {
        Some(current) => {
            let _ = write!(
                question,
                ", {} its branch {}",
                if current_ticked {
                    "deleting"
                } else {
                    "keeping"
                },
                current.name
            );
            box_over(&[current], state(current_ticked))
        }
        None if offer.protected.contains(&offer.current_name) => not_offered(),
        None if delete_branch == Some(true) => {
            return Err(CliError::new(
                Exit::Failed,
                format!(
                    "dux could not read the branch agent {agent} is on, so nothing was deleted"
                ),
            ));
        }
        None => {
            question.push_str(", keeping its branch");
            if !offer.current_name.is_empty() {
                let _ = write!(question, " {}", offer.current_name);
            }
            crate::branch_history::BoxWire {
                unchecked: Some(crate::branch_history::Unchecked::Failed),
                ..crate::branch_history::BoxWire::default()
            }
        }
    };
    let others_ticked = delete_other_branches.unwrap_or_else(|| offer.others_default());
    let others = if offer.others.is_empty() {
        if delete_other_branches == Some(true) {
            notes.push("It was on no other branch to delete.".to_string());
        }
        not_offered()
    } else {
        let names: Vec<&str> = offer.others.iter().map(|o| o.name.as_str()).collect();
        let _ = write!(
            question,
            ", {} the other {} it was on: {}",
            if others_ticked { "deleting" } else { "keeping" },
            if names.len() == 1 {
                "branch"
            } else {
                "branches"
            },
            names.join(", ")
        );
        let others: Vec<&crate::branch_history::OfferedBranch> = offer.others.iter().collect();
        box_over(&others, state(others_ticked))
    };
    // The project's own branches the agent was on, which the dialogs name as
    // kept too.
    if !offer.protected.is_empty() {
        let _ = write!(
            question,
            ", keeping the project's own {} {}",
            if offer.protected.len() == 1 {
                "branch"
            } else {
                "branches"
            },
            offer.protected.join(", ")
        );
    }
    notes.extend(loss_note(offer, &current, &others));
    Ok(BranchAnswersPlan {
        answers: crate::branch_history::BranchAnswersWire {
            current: Some(current),
            others: Some(others),
        },
        question,
        notice: (!notes.is_empty()).then(|| notes.join(" ")),
    })
}

/// `dux agents rm <a>`. `delete_branch` answers for the branch the worktree
/// is on and `delete_other_branches` for every other branch it has been on, as
/// the delete dialogs' two boxes do.
///
/// Any branch flag makes the command read the branches from the running dux
/// first and answer BOTH boxes as the dialogs would, the box no flag named at
/// its default, so the question names every branch that goes and every branch
/// that stays, and the line before it, there only when deleting them loses
/// any commit, says how many. A
/// standalone agent has no branch of its own, and a worktree that stays keeps
/// every branch, so neither reads anything. With no flag
/// at all, only the branch dux made for the agent goes, and only while dux can
/// prove it.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, `query` names no single
/// agent, or a branch it was asked to delete cannot be read.
pub fn agents_rm(
    client: &Client,
    query: &str,
    worktree: Worktrees,
    delete_branch: Option<bool>,
    delete_other_branches: Option<bool>,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let delete_worktree = worktree.deleted();
    let agent = find_agent(client, query)?;
    let mut question = format!("Delete agent {}", agent.name());
    let mut path = format!(
        "/api/v1/sessions/{}?delete_worktree={delete_worktree}",
        segment(&agent.id)
    );
    if delete_worktree {
        let _ = write!(question, " and its worktree {}", agent.directory());
    }
    let flagged = delete_branch.is_some() || delete_other_branches.is_some();
    let managed = matches!(agent.workspace, Workspace::Managed { .. });
    if !flagged || !managed || !delete_worktree {
        // The older single answer, exactly as before the boxes: with no flag
        // the server decides, a standalone agent has no branch of its own, and
        // a worktree that stays keeps every branch, as the dialogs offer no
        // box for it.
        let answer = match (delete_branch, delete_other_branches) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, None) => None,
        };
        match answer {
            Some(true) => question.push_str(", deleting its branch"),
            Some(false) => question.push_str(", keeping its branch"),
            None => {}
        }
        if let Some(delete) = answer {
            let _ = write!(path, "&delete_branch={delete}");
        }
        return Ok(Planned::new(question, Method::Delete, path, None).forced(if_attached));
    }
    let offer: BranchOfferReply = client.get_json(&format!(
        "/api/v1/sessions/{}/branch-unpushed",
        segment(&agent.id)
    ))?;
    let plan = plan_branch_answers(&offer, delete_branch, delete_other_branches, &agent.name())?;
    question.push_str(&plan.question);
    let body = json!({ "branches": plan.answers });
    let mut planned = Planned::new(question, Method::Delete, path, Some(body)).forced(if_attached);
    planned.notice = plan.notice;
    Ok(planned)
}

/// `dux agents stop <a>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent.
pub fn agents_stop(
    client: &Client,
    query: &str,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let agent = find_agent(client, query)?;
    Ok(Planned::new(
        format!("Stop everything agent {} runs", agent.name()),
        Method::Post,
        format!("/api/v1/sessions/{}/kill", segment(&agent.id)),
        None,
    )
    .forced(if_attached))
}

/// `dux agents start <a>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent.
pub fn agents_start(client: &Client, query: &str) -> Result<Planned, CliError> {
    let agent = find_agent(client, query)?;
    Ok(Planned::new(
        format!("Start agent {}", agent.name()),
        Method::Post,
        format!("/api/v1/sessions/{}/reconnect", segment(&agent.id)),
        Some(json!({ "force": false })),
    ))
}

/// `dux agents tabs add <a>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent.
pub fn tabs_add(client: &Client, query: &str, provider: Option<&str>) -> Result<Planned, CliError> {
    let agent = find_agent(client, query)?;
    let what = provider.map_or("a tab".to_string(), |p| format!("a {p} tab"));
    Ok(Planned::new(
        format!("Add {what} to agent {}", agent.name()),
        Method::Post,
        format!("/api/v1/sessions/{}/tabs", segment(&agent.id)),
        Some(json!({ "provider": provider })),
    ))
}

/// What a tab verb acts on: the question's words and the tab's address.
fn tab_address(
    client: &Client,
    agent: &str,
    tab: &str,
) -> Result<(String, String, String), CliError> {
    let agent = find_agent(client, agent)?;
    let (tab, label) = find_tab(&agent, tab)?;
    Ok((
        agent.name(),
        label,
        format!(
            "/api/v1/sessions/{}/tabs/{}",
            segment(&agent.id),
            segment(&tab.id)
        ),
    ))
}

/// `dux agents tabs rm <a> <tab>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or the agent or tab is not named by exactly
/// one.
pub fn tabs_rm(
    client: &Client,
    agent: &str,
    tab: &str,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let (agent, label, path) = tab_address(client, agent, tab)?;
    Ok(Planned::new(
        format!("Close the {label} tab of agent {agent}"),
        Method::Delete,
        path,
        None,
    )
    .forced(if_attached))
}

/// `dux agents tabs start <a> <tab>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or the agent or tab is not named by exactly
/// one.
pub fn tabs_start(client: &Client, agent: &str, tab: &str) -> Result<Planned, CliError> {
    let (agent, label, path) = tab_address(client, agent, tab)?;
    Ok(Planned::new(
        format!("Start the {label} tab of agent {agent}"),
        Method::Post,
        format!("{path}/start"),
        None,
    ))
}

/// `dux agents tabs stop <a> <tab>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or the agent or tab is not named by exactly
/// one.
pub fn tabs_stop(
    client: &Client,
    agent: &str,
    tab: &str,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let (agent, label, path) = tab_address(client, agent, tab)?;
    Ok(Planned::new(
        format!("Stop the {label} tab of agent {agent}"),
        Method::Post,
        format!("{path}/stop"),
        None,
    )
    .forced(if_attached))
}

/// `dux agents prs attach <a> <ref>`. The reference goes as typed: the dux
/// resolves a bare number against the agent's own repository.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent or a
/// standalone one.
pub fn prs_attach(client: &Client, query: &str, reference: &str) -> Result<Planned, CliError> {
    let agent = pr_agent(client, query)?;
    Ok(Planned::new(
        format!("Attach pull request {reference} to agent {}", agent.name()),
        Method::Post,
        format!("/api/v1/sessions/{}/pull-requests", segment(&agent.id)),
        Some(json!({ "pr": reference })),
    ))
}

/// The agent `query` names, refused before anything is asked when it is a
/// standalone one, which has no pull requests, in the sentence the dux
/// itself refuses with.
fn pr_agent(client: &Client, query: &str) -> Result<Agent, CliError> {
    let agent = find_agent(client, query)?;
    match agent.workspace {
        Workspace::Managed { .. } => Ok(agent),
        Workspace::Folder { .. } => Err(CliError::new(
            Exit::Usage,
            crate::engine::standalone_agent_refusal(
                &agent.name(),
                crate::engine::pull_requests::PR_FEATURE,
                crate::engine::STANDALONE_ADD_AS_PROJECT_REMEDY,
            ),
        )),
    }
}

/// The agent a pull-request verb acts on and the pull request it names.
fn pr_address(client: &Client, query: &str, reference: &str) -> Result<(Agent, Pr), CliError> {
    let agent = pr_agent(client, query)?;
    let pr = find_pr(&agent, reference)?.clone();
    Ok((agent, pr))
}

/// A change to one of `agent`'s pull requests, at `route` under its
/// pull-request address.
fn pr_change(agent: &Agent, pr: &Pr, question: String, route: &str) -> Planned {
    Planned::new(
        question,
        Method::Post,
        format!(
            "/api/v1/sessions/{}/pull-requests/{route}",
            segment(&agent.id)
        ),
        Some(pr.key()),
    )
}

/// `dux agents prs detach <a> <ref>`. One dux only found has nothing to
/// detach, so it is refused before anything is asked, naming the verb that
/// does what the user wants.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, the agent or the pull
/// request is not named by exactly one, or the pull request was not attached.
pub fn prs_detach(client: &Client, query: &str, reference: &str) -> Result<Planned, CliError> {
    let (agent, pr) = pr_address(client, query, reference)?;
    let name = agent.name();
    if !pr.attached {
        let label = pr.label();
        return Err(CliError::new(
            Exit::Usage,
            format!(
                "{label} on {name} was found by dux, not attached; hide it instead (dux agents \
                 prs hide {name} {})",
                pr.url
            ),
        ));
    }
    let question = format!(
        "Detach pull request #{} ({}) from agent {name}",
        pr.number, pr.owner_repo
    );
    Ok(pr_change(&agent, &pr, question, "detach"))
}

/// `dux agents prs hide <a> <ref>`. One only attached is refused before
/// anything is asked: hiding is for what dux found.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, the agent or the pull
/// request is not named by exactly one, or the pull request was only attached.
pub fn prs_hide(client: &Client, query: &str, reference: &str) -> Result<Planned, CliError> {
    let (agent, pr) = pr_address(client, query, reference)?;
    let name = agent.name();
    let label = pr.label();
    if !pr.found {
        return Err(CliError::new(
            Exit::Usage,
            format!(
                "{label} on {name} was attached, not found by dux; detach it instead (dux agents \
                 prs detach {name} {})",
                pr.url
            ),
        ));
    }
    let question = format!("Hide pull request {label} on agent {name}");
    Ok(pr_change(&agent, &pr, question, "hide"))
}

/// `dux agents prs unhide <a> <ref>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or the agent or the pull
/// request is not named by exactly one.
pub fn prs_unhide(client: &Client, query: &str, reference: &str) -> Result<Planned, CliError> {
    let (agent, pr) = pr_address(client, query, reference)?;
    let question = format!(
        "Show pull request {} again on agent {}",
        pr.label(),
        agent.name()
    );
    Ok(pr_change(&agent, &pr, question, "show"))
}

/// `dux agents prs sort <a> <mode>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single agent or a
/// standalone one.
pub fn prs_sort(
    client: &Client,
    query: &str,
    mode: crate::wire::PullRequestSort,
) -> Result<Planned, CliError> {
    let agent = pr_agent(client, query)?;
    let order = match mode {
        crate::wire::PullRequestSort::Automatic => "automatic order",
        crate::wire::PullRequestSort::Manual => "your own order",
    };
    Ok(Planned::new(
        format!("Put agent {}'s pull requests in {order}", agent.name()),
        Method::Put,
        format!("/api/v1/sessions/{}/pull-requests/sort", segment(&agent.id)),
        Some(json!({ "mode": mode })),
    ))
}

/// `dux agents prs move <a> <ref> <to>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, the agent or the pull
/// request is not named by exactly one, the agent is a standalone one, or the
/// pull request is hidden or `to` is a position past the ones it shows.
pub fn prs_move(
    client: &Client,
    query: &str,
    reference: &str,
    to: crate::wire::PullRequestMove,
) -> Result<Planned, CliError> {
    use crate::wire::{PullRequestMove, PullRequestMoveNamed};
    let (agent, pr) = pr_address(client, query, reference)?;
    let name = agent.name();
    if !agent.prs.iter().any(|shown| shown.url == pr.url) {
        return Err(CliError::new(
            Exit::Usage,
            crate::engine::pull_requests::hidden_move_refusal(&pr.label(), &name),
        ));
    }
    if let PullRequestMove::Position(n) = to
        && (n == 0 || n > agent.prs.len())
    {
        return Err(CliError::new(
            Exit::Usage,
            crate::engine::pull_requests::position_out_of_range_refusal(&name, agent.prs.len()),
        ));
    }
    let place = match to {
        PullRequestMove::Named(PullRequestMoveNamed::Top) => "to the top".to_string(),
        PullRequestMove::Named(PullRequestMoveNamed::Up) => "up".to_string(),
        PullRequestMove::Named(PullRequestMoveNamed::Down) => "down".to_string(),
        PullRequestMove::Position(n) => format!("to position {n}"),
    };
    Ok(Planned::new(
        format!("Move pull request {} {place} on agent {name}", pr.label()),
        Method::Post,
        format!("/api/v1/sessions/{}/pull-requests/move", segment(&agent.id)),
        Some(json!({ "pr": pr.key(), "to": to })),
    ))
}

/// Who a new terminal belongs to.
#[derive(Debug)]
pub enum TerminalFor {
    /// An agent's companion terminal, by the agent's id or name.
    Agent(String),
    /// A project terminal, by the project's id or name.
    Project(String),
    /// A terminal that belongs to nothing.
    Standalone,
}

/// `dux terminals add`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or the owner named names no single agent or
/// project.
pub fn terminals_add(client: &Client, owner: TerminalFor) -> Result<Planned, CliError> {
    let (question, path) = match owner {
        TerminalFor::Agent(query) => {
            let agent = find_agent(client, &query)?;
            (
                format!("Open a terminal in agent {}'s worktree", agent.name()),
                format!("/api/v1/sessions/{}/terminals", segment(&agent.id)),
            )
        }
        TerminalFor::Project(query) => {
            let projects = projects(client)?;
            let project = find_project(&projects, &query)?;
            (
                format!("Open a terminal in project {}", project.name),
                format!("/api/v1/projects/{}/terminals", segment(&project.id)),
            )
        }
        TerminalFor::Standalone => (
            "Open a standalone terminal in the home folder".to_string(),
            "/api/v1/terminals".to_string(),
        ),
    };
    Ok(Planned::new(question, Method::Post, path, None))
}

/// `dux terminals rm <t>`.
///
/// # Errors
///
/// Fails when the dux cannot be reached or refuses, or `query` names no single terminal.
pub fn terminals_rm(
    client: &Client,
    query: &str,
    if_attached: IfAttached,
) -> Result<Planned, CliError> {
    let document = WorkspaceDocument::read(client)?;
    let terminal = output::select(
        "terminal",
        query,
        &document.terminals,
        |t| &t.id,
        |t| &t.label,
    )?;
    let id = segment(&terminal.id);
    let path = match &terminal.owner {
        Owner::Session { session_id } => {
            format!("/api/v1/sessions/{}/terminals/{id}", segment(session_id))
        }
        Owner::Project { project_id } => {
            format!("/api/v1/projects/{}/terminals/{id}", segment(project_id))
        }
        Owner::Standalone { .. } => format!("/api/v1/terminals/{id}"),
    };
    Ok(Planned::new(
        format!(
            "Close terminal {} ({})",
            terminal.label,
            owner_words(&terminal.owner, &document)
        ),
        Method::Delete,
        path,
        None,
    )
    .forced(if_attached))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_naming_an_existing_branch_says_how_to_go_ahead() {
        let refused = existing_branch_refusal(CliError::new(
            Exit::Refused,
            r#"{"existing_branch":{"name":"feat-a","location":"local"}}"#,
        ));
        assert_eq!(refused.exit, Exit::Refused);
        assert!(refused.message.contains("feat-a"), "{}", refused.message);
        assert!(
            refused.message.contains("--existing-branch"),
            "{}",
            refused.message
        );
        let other = CliError::new(
            Exit::Refused,
            "An agent is already being created or forked.",
        );
        assert_eq!(existing_branch_refusal(other.clone()), other);
    }
}

#[cfg(test)]
mod create_wait_tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, private_dir};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SENTENCE: &str = "An agent is already being created or forked.";
    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;
    const OTHER_RUNNING: &str = r#"{"id":"op-1","kind":"agent.create","state":"running","message":"","created":[],"removed":[],"parts":[]}"#;
    const OTHER_DONE: &str = r#"{"id":"op-1","kind":"agent.create","state":"succeeded","message":"Created agent first.","created":["s1"],"removed":[],"parts":[]}"#;
    const MINE_RUNNING: &str = r#"{"id":"op-2","kind":"agent.create","state":"running","message":"","created":[],"removed":[],"parts":[]}"#;
    const MINE_DONE: &str = r#"{"id":"op-2","kind":"agent.create","state":"succeeded","message":"Created agent second.","created":["s2"],"removed":[],"parts":[]}"#;
    const NAMED: &str =
        r#"{"message":"An agent is already being created or forked.","operation":"op-1"}"#;

    /// What the stand-in answers: the n-th create POST gets `refusals[n]` (later ones are
    /// accepted with `accepted`), and the n-th read of operation op-1 gets `other[n]`
    /// (the last repeats).
    struct Script {
        refusals: Vec<&'static str>,
        other: Vec<(u16, &'static str)>,
        accepted: &'static str,
    }

    impl Script {
        fn new(refusals: Vec<&'static str>) -> Self {
            Self {
                refusals,
                other: vec![(200, OTHER_DONE)],
                accepted: MINE_DONE,
            }
        }
    }

    fn dux_running(
        dir: &Path,
        script: Script,
    ) -> (FakeDux, crate::lockfile::SingleInstanceLock, Client) {
        let socket = dir.join("dux.sock");
        let posts = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let fake = FakeDux::unix(&socket, move |seen| {
            if seen.path == "/api/v1/build" {
                return Reply::json(200, BUILD);
            }
            if seen.path.starts_with("/api/v1/operations/op-1") {
                let n = reads.fetch_add(1, Ordering::SeqCst);
                let (status, body) = script.other[n.min(script.other.len() - 1)];
                return Reply::json(status, body);
            }
            if seen.path.starts_with("/api/v1/operations/op-2") {
                return Reply::json(200, MINE_RUNNING);
            }
            if seen.path == "/api/v1/projects" {
                return Reply::json(200, r#"[{"id":"p1","name":"app","path":"/src/app"}]"#);
            }
            if seen.path == "/api/v1/sessions?operation=1" {
                let n = posts.fetch_add(1, Ordering::SeqCst);
                return match script.refusals.get(n) {
                    Some(body) => Reply::json(409, body),
                    None => Reply::json(202, script.accepted),
                };
            }
            Reply::json(404, "{}")
        });
        let lock_path = dir.join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        (fake, lock, client)
    }

    fn add(client: &Client) -> Planned {
        agents_add(
            client,
            NewAgent::Standalone {
                folder: "/work/f".to_string(),
                provider: None,
            },
            Some("second"),
            Path::new("/"),
        )
        .unwrap()
    }

    fn posts(fake: &FakeDux) -> usize {
        fake.seen()
            .iter()
            .filter(|seen| seen.path == "/api/v1/sessions?operation=1")
            .count()
    }

    #[test]
    fn a_pull_request_create_carries_the_agreement_to_a_review_copy() {
        let dir = private_dir();
        let (_fake, _lock, client) = dux_running(dir.path(), Script::new(vec![]));
        let body_for = |review_copy| {
            agents_add(
                &client,
                NewAgent::PullRequest {
                    project: "app".to_string(),
                    reference: "42".to_string(),
                    review_copy,
                },
                None,
                Path::new("/"),
            )
            .unwrap()
            .body
            .unwrap()
        };
        assert_eq!(body_for(ReviewCopy::Agreed)["review_copy"], true);
        assert_eq!(body_for(ReviewCopy::NotAgreed)["review_copy_by_flag"], true);
        assert_eq!(body_for(ReviewCopy::NotAgreed)["review_copy"], false);
    }

    #[test]
    fn a_create_refused_for_another_create_waits_for_it_and_tries_again() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED]));
        let out = perform(&client, &add(&client), Some(Duration::from_secs(5))).unwrap();
        assert_eq!(out, "Created agent second.\ns2\n");
        assert_eq!(posts(&fake), 2);
        assert!(
            fake.seen()
                .iter()
                .any(|seen| seen.path.starts_with("/api/v1/operations/op-1")),
            "the other creation's operation was read"
        );
    }

    #[test]
    fn a_create_refused_for_another_create_without_an_operation_is_retried_until_accepted() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![SENTENCE, SENTENCE]));
        let out = perform(&client, &add(&client), Some(Duration::from_secs(10))).unwrap();
        assert_eq!(out, "Created agent second.\ns2\n");
        assert_eq!(posts(&fake), 3);
    }

    #[test]
    fn with_no_wait_a_create_refused_for_another_create_is_not_retried() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED]));
        let error = perform(&client, &add(&client), None).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, SENTENCE);
        assert_eq!(posts(&fake), 1);
    }

    #[test]
    fn a_create_refused_again_after_waiting_fails_with_the_refusal() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED, NAMED]));
        let error = perform(&client, &add(&client), Some(Duration::from_secs(5))).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, SENTENCE);
        assert_eq!(posts(&fake), 2);
    }

    #[test]
    fn the_other_creation_counts_as_finished_only_when_the_dux_has_forgotten_it() {
        // Forgotten (404): finished, so the create goes ahead.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(404, r#"{"error":"unknown_operation"}"#)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        assert!(perform(&client, &add(&client), Some(Duration::from_secs(5))).is_ok());
        assert_eq!(posts(&fake), 2);

        // A server error is read again within the time, then the answer counts.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(500, "boom"), (200, OTHER_DONE)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        assert!(perform(&client, &add(&client), Some(Duration::from_secs(5))).is_ok());
        assert_eq!(posts(&fake), 2);

        // Any other refusal of the read is the command's error, and nothing is sent again.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(400, r#"{"error":"bad request"}"#)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        let error = perform(&client, &add(&client), Some(Duration::from_secs(5))).unwrap_err();
        assert_eq!(error.exit, Exit::Failed);
        assert_eq!(error.message, "bad request");
        assert_eq!(posts(&fake), 1);
    }

    #[test]
    fn waiting_for_the_other_creation_comes_out_of_the_one_time_the_command_has() {
        // The other creation takes about 0.75 s of a 3 s wait; the new agent then
        // never finishes. Its wait gets what is left, so the command ends near
        // 3 s in all, not near 3.75 s. The spare seconds absorb a slow machine.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![
            (200, OTHER_RUNNING),
            (200, OTHER_RUNNING),
            (200, OTHER_RUNNING),
            (200, OTHER_DONE),
        ];
        script.accepted = MINE_RUNNING;
        let (_fake, _lock, client) = dux_running(dir.path(), script);
        let started = Instant::now();
        let error = perform(&client, &add(&client), Some(Duration::from_secs(3))).unwrap_err();
        assert_eq!(error.exit, Exit::Unknown);
        assert!(
            started.elapsed() < Duration::from_millis(3600),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn no_create_is_started_once_the_time_has_run_out() {
        // The other creation outlasts the 1 s wait: this one never started, so the
        // command ends refused, with the refusal, and sends no second create.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(200, OTHER_RUNNING)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        let error = perform(&client, &add(&client), Some(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, SENTENCE);
        assert_eq!(posts(&fake), 1);

        // The same with no operation named: it gives up with the refusal.
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![SENTENCE; 100]));
        let started = Instant::now();
        let error = perform(&client, &add(&client), Some(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert!(started.elapsed() < Duration::from_millis(1400));
        assert!(posts(&fake) <= 3, "{} creates", posts(&fake));
    }
}

#[cfg(test)]
mod pull_request_anywhere_tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;
    const NEEDED: &str = r#"{"state":"needed","reason":"busy","branch":"fix","copy_branch":"fix-review","warning":["Branch ",{"name":"fix","quoted":true}," is busy, so the agent would work on a review copy."]}"#;
    const NO_NAME: &str = r#"{"state":"no_free_name","reason":"busy","branch":"fix","message":["Every review name is taken. Type another name for the agent from PR #5."]}"#;
    const NOT_NEEDED: &str = r#"{"state":"not_needed"}"#;
    const FREE_FOLDER: &str = r#"{"state":"free","path":"/srv/src/other"}"#;
    const CHECKOUT_NEEDED: &str = r#"{"state":"checkout","path":"/work/other","review_copy":{"state":"needed","reason":"diverged","branch":"fix","copy_branch":"fix-review","warning":["Folder copy needed."]}}"#;
    const CHECKOUT_PLAIN: &str = r#"{"state":"checkout","path":"/work/other"}"#;

    fn resolve_reply(projects: &str, clone: Option<&str>, unavailable: Option<&str>) -> String {
        let clone = clone
            .map(|review| {
                format!(
                    r#","clone":{{"address":"git@github.com:octo/other.git","host":"github.com","owner_repo":"octo/other","number":5,"title":"T","state":"OPEN","head_branch":"fix","head_oid":"abc","review_copy":{review}}}"#
                )
            })
            .unwrap_or_default();
        let unavailable = unavailable
            .map(|why| format!(r#","clone_unavailable":"{why}""#))
            .unwrap_or_default();
        format!(
            r#"{{"repository":"github.com/octo/other","number":5,"projects":{projects},"uninspected_count":0,"uninspected_summary":null{clone}{unavailable}}}"#
        )
    }

    /// A dux whose resolve answers `resolve`, whose folder check answers
    /// `folder`, and whose start folder is /srv/src.
    fn dux(resolve: String, folder: &'static str) -> Running {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            "/api/v1/pull-requests/resolve" => Reply::json(200, &resolve),
            "/api/v1/pull-requests/clone-destination" => Reply::json(200, folder),
            "/api/v1/browse" => Reply::json(200, r#"{"path":"/srv/src","entries":[]}"#),
            "/api/v1/projects" => {
                Reply::json(200, r#"[{"id":"p1","name":"app","path":"/src/app"}]"#)
            }
            _ => Reply::json(404, "{}"),
        });
        let lock_path = dir.path().join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        Running {
            fake,
            client,
            _lock: lock,
            _dir: dir,
        }
    }

    struct Running {
        fake: FakeDux,
        client: Client,
        _lock: crate::lockfile::SingleInstanceLock,
        _dir: tempfile::TempDir,
    }

    impl Running {
        fn plan(
            &self,
            path: Option<&str>,
            review_copy: ReviewCopy,
            name: Option<&str>,
        ) -> Result<Planned, CliError> {
            self.plan_for("octo/other#5", path, review_copy, name)
        }

        fn plan_for(
            &self,
            reference: &str,
            path: Option<&str>,
            review_copy: ReviewCopy,
            name: Option<&str>,
        ) -> Result<Planned, CliError> {
            agents_add(
                &self.client,
                NewAgent::PullRequestAnywhere {
                    reference: reference.to_string(),
                    path: path.map(str::to_string),
                    review_copy,
                },
                name,
                Path::new("/"),
            )
        }
    }

    fn free(review: &str) -> String {
        resolve_reply("[]", Some(review), None)
    }

    #[test]
    fn a_bare_number_is_refused_before_anything_is_sent() {
        let dux = dux(free(NOT_NEEDED), FREE_FOLDER);
        let error = dux
            .plan_for(" #42 ", None, ReviewCopy::NotAgreed, None)
            .err()
            .unwrap();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(
            error.message,
            "A pull request number on its own does not say which repository it is in. Pass a link or owner/repo#123, or add --project to choose one."
        );
        assert!(
            dux.fake
                .seen()
                .iter()
                .all(|seen| seen.path == "/api/v1/build")
        );
    }

    #[test]
    fn a_free_folder_is_cloned_into_the_start_folder_and_the_flag_is_not_given() {
        let dux = dux(free(NOT_NEEDED), FREE_FOLDER);
        let planned = dux.plan(None, ReviewCopy::NotAgreed, Some("mine")).unwrap();
        assert_eq!(
            planned.question,
            "Clone git@github.com:octo/other.git into /srv/src/other and create agent mine from pull request octo/other#5"
        );
        assert_eq!(planned.notice, None);
        let body = planned.body.unwrap();
        assert_eq!(body["path"], "/srv/src/other");
        assert_eq!(body["agent_name"], "mine");
        assert_eq!(body["add_existing"], false);
        assert_eq!(
            body["pull_request"],
            json!({"host":"github.com","owner_repo":"octo/other","number":5})
        );
        assert_eq!(body["review_copy_by_flag"], true);
        assert_eq!(body["review_copy"], false);
        let check = dux
            .fake
            .seen()
            .into_iter()
            .find(|seen| seen.path.ends_with("clone-destination"))
            .unwrap();
        let sent: Value = serde_json::from_str(&check.body).unwrap();
        assert_eq!(sent["path"], "/srv/src/other");
        assert_eq!(sent["head_branch"], "fix");
        assert_eq!(sent["head_oid"], "abc");
        assert_eq!(sent["number"], 5);
    }

    #[test]
    fn an_unnamed_agent_omits_its_name_and_a_checkout_is_added() {
        let dux = dux(free(NOT_NEEDED), CHECKOUT_PLAIN);
        let planned = dux
            .plan(Some("/work/other"), ReviewCopy::NotAgreed, None)
            .unwrap();
        assert_eq!(
            planned.question,
            "Add the folder /work/other as a project and create an agent from pull request octo/other#5"
        );
        let body = planned.body.unwrap();
        assert_eq!(body["add_existing"], true);
        assert!(body.get("agent_name").is_none_or(Value::is_null));
    }

    #[test]
    fn a_needed_review_copy_is_refused_without_the_flag_and_said_with_it() {
        let dux = dux(free(NEEDED), FREE_FOLDER);
        let refused = dux.plan(None, ReviewCopy::NotAgreed, None).err().unwrap();
        assert_eq!(refused.exit, Exit::Refused);
        assert_eq!(
            refused.message,
            "Branch \"fix\" is busy, so the agent would work on a review copy. To go ahead anyway, run the command again with --review-copy. Nothing was cloned."
        );
        let agreed = dux.plan(None, ReviewCopy::Agreed, Some("fix")).unwrap();
        assert_eq!(
            agreed.notice.as_deref(),
            Some("Branch \"fix\" is busy, so the agent would work on a review copy.")
        );
        assert_eq!(agreed.body.as_ref().unwrap()["review_copy"], true);
    }

    #[test]
    fn a_checkout_answer_replaces_the_offers_and_words_the_add() {
        // The checkout says nothing about a copy, so the offer's warning no
        // longer applies.
        let plain = dux(free(NEEDED), CHECKOUT_PLAIN);
        assert!(
            plain
                .plan(Some("/work/other"), ReviewCopy::NotAgreed, None)
                .is_ok()
        );
        let needed = dux(free(NOT_NEEDED), CHECKOUT_NEEDED);
        let refused = needed
            .plan(Some("/work/other"), ReviewCopy::NotAgreed, None)
            .err()
            .unwrap();
        assert_eq!(
            refused.message,
            "Folder copy needed. To go ahead anyway, run the command again with --review-copy. Nothing was added."
        );
    }

    #[test]
    fn a_typed_name_skips_the_review_copy_rules_and_no_free_name_refuses() {
        let needed = dux(free(NEEDED), FREE_FOLDER);
        assert!(
            needed
                .plan(None, ReviewCopy::NotAgreed, Some("other-name"))
                .is_ok()
        );
        let none = dux(free(NO_NAME), FREE_FOLDER);
        let refused = none.plan(None, ReviewCopy::Agreed, None).err().unwrap();
        assert_eq!(
            refused.message,
            "Every review name is taken. Type another name for the agent from PR #5. Nothing was cloned."
        );
        assert!(
            none.plan(None, ReviewCopy::NotAgreed, Some("elsewhere"))
                .is_ok()
        );
    }

    #[test]
    fn a_typed_name_spelled_like_a_copy_is_asked_about_and_names_are_trimmed() {
        let clone = dux(free(NOT_NEEDED), FREE_FOLDER);
        let refused = clone
            .plan(None, ReviewCopy::NotAgreed, Some(" fix-review "))
            .err()
            .unwrap();
        assert_eq!(refused.exit, Exit::Refused);
        assert!(
            refused.message.ends_with(
                " To go ahead anyway, run the command again with --review-copy. Nothing was cloned."
            ),
            "{}",
            refused.message
        );
        let agreed = clone
            .plan(None, ReviewCopy::Agreed, Some("fix-review"))
            .unwrap();
        assert!(agreed.notice.is_some());
        // Spaces around the branch's own name do not make it another name.
        let offered = dux(free(NEEDED), FREE_FOLDER);
        assert!(
            offered
                .plan(None, ReviewCopy::NotAgreed, Some(" fix "))
                .is_err()
        );
        // An add cannot tell whether a branch of that name exists, so the
        // server asks.
        let add = dux(free(NOT_NEEDED), CHECKOUT_PLAIN);
        assert!(
            add.plan(
                Some("/work/other"),
                ReviewCopy::NotAgreed,
                Some("fix-review")
            )
            .is_ok()
        );
    }

    #[test]
    fn an_incomplete_check_of_projects_is_said_before_the_clone_is_asked() {
        let reply = resolve_reply("[]", Some(NOT_NEEDED), None).replace(
            r#""uninspected_summary":null"#,
            r#""uninspected_summary":"1 has an address dux could not read""#,
        );
        let dux = dux(reply, FREE_FOLDER);
        let planned = dux.plan(None, ReviewCopy::NotAgreed, None).unwrap();
        assert_eq!(
            planned.notice.as_deref(),
            Some(
                "dux could not find a checked project for github.com/octo/other. dux could not check every project (1 has an address dux could not read), so one of them may already be a checkout of it. dux will clone it, add it as a project, and create the agent from pull request #5."
            )
        );
    }

    #[test]
    fn matching_projects_and_a_failed_lookup_have_their_own_refusals() {
        let one = dux(
            resolve_reply(r#"[{"id":"p1","name":"app"}]"#, None, None),
            FREE_FOLDER,
        );
        let planned = one.plan(None, ReviewCopy::NotAgreed, None).unwrap();
        assert_eq!(
            planned.question,
            "Create an agent from pull request octo/other#5 in project app"
        );
        assert_eq!(planned.body.unwrap()["project_id"], "p1");
        // The folder of the project itself is not a different folder.
        assert!(
            one.plan(Some("/src/app"), ReviewCopy::NotAgreed, None)
                .is_ok()
        );
        let refused = one
            .plan(Some("/x"), ReviewCopy::NotAgreed, None)
            .err()
            .unwrap();
        assert_eq!(
            refused.message,
            "github.com/octo/other is already a checkout in project app, so there is nothing to clone or add at /x. Drop --path to create the agent in that project."
        );

        let two = dux(
            resolve_reply(
                r#"[{"id":"p1","name":"app"},{"id":"p2","name":"app2"}]"#,
                None,
                None,
            ),
            FREE_FOLDER,
        );
        let refused = two.plan(None, ReviewCopy::NotAgreed, None).err().unwrap();
        assert_eq!(
            refused.message,
            "github.com/octo/other is checked out in more than one project (app, app2); add --project to choose one."
        );

        let none = dux(
            resolve_reply("[]", None, Some("Failed to resolve PR #5.")),
            FREE_FOLDER,
        );
        let refused = none.plan(None, ReviewCopy::NotAgreed, None).err().unwrap();
        assert_eq!(
            refused.message,
            "No project in dux is a checkout of github.com/octo/other. dux could not look the pull request up to offer a clone: Failed to resolve PR #5. Add --project to choose one."
        );
        let unpunctuated = dux(
            resolve_reply("[]", None, Some("gh did not answer")),
            FREE_FOLDER,
        );
        let refused = unpunctuated
            .plan(None, ReviewCopy::NotAgreed, None)
            .err()
            .unwrap();
        assert!(
            refused
                .message
                .ends_with("gh did not answer. Add --project to choose one."),
            "{}",
            refused.message
        );
    }
}

#[cfg(test)]
mod agents_rm_tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;
    const AGENTS: &str = r#"[{"id":"a1","title":"feat","provider":"claude","status":"detached","workspace":{"kind":"managed","project_id":"p1","branch_name":"main","worktree_path":"/wt/feat"}}]"#;

    /// `dux agents rm` against a dux whose branch offer answers `offer`.
    fn plan(offer: &'static str, delete: Option<bool>, others: Option<bool>) -> Planned {
        plan_with(offer, Worktrees::Delete, delete, others)
    }

    fn plan_with(
        offer: &'static str,
        worktree: Worktrees,
        delete: Option<bool>,
        others: Option<bool>,
    ) -> Planned {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            "/api/v1/sessions" => Reply::json(200, AGENTS),
            "/api/v1/sessions/a1/branch-unpushed" => Reply::json(200, offer),
            _ => Reply::json(404, "{}"),
        });
        let lock_path = dir.path().join("dux.lock");
        let _lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        agents_rm(
            &client,
            "feat",
            worktree,
            delete,
            others,
            IfAttached::Refuse,
        )
        .unwrap()
    }

    /// The project's own branch is offered in no box: the command line says
    /// so, sends what the dialogs send, and deletes nothing it would cover.
    #[test]
    fn delete_branch_on_the_projects_own_branch_says_so_and_sends_not_offered() {
        let planned = plan(
            r#"{"current_name":"main","current":null,"others":[],"protected":["main"]}"#,
            Some(true),
            Some(true),
        );
        assert!(
            planned
                .question
                .ends_with(", keeping the project's own branch main"),
            "{}",
            planned.question
        );
        let body = planned.body.unwrap();
        assert_eq!(body["branches"]["current"]["unchecked"], "not_offered");
        assert_eq!(body["branches"]["others"]["unchecked"], "not_offered");
        assert_eq!(
            planned.notice.as_deref(),
            Some("It was on no other branch to delete."),
            "nothing is deleted, so nothing is lost"
        );
    }

    /// The agent on its dux-made `agent-x` (3 commits of its own) that also
    /// visited `spike`: `--delete-other-branches` alone leaves the first box at
    /// the default the dialogs show, ticked, so the question names `agent-x`
    /// too and the line before it counts what deleting both loses.
    #[test]
    fn one_delete_flag_answers_the_other_box_at_the_dialogs_default() {
        let planned = plan(
            r#"{"current_name":"agent-x","current":{"name":"agent-x","oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":"dux"},
               "current_unpushed":{"count":3,"has_remote_refs":true},
               "others":[{"name":"spike","oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","origin":"after"}],
               "others_unpushed":{"count":1,"has_remote_refs":true},
               "both_unpushed":{"count":4,"has_remote_refs":true},"protected":[]}"#,
            None,
            Some(true),
        );
        assert!(
            planned.question.contains(", deleting its branch agent-x")
                && planned
                    .question
                    .contains(", deleting the other branch it was on: spike"),
            "{}",
            planned.question
        );
        assert_eq!(
            planned.notice.as_deref(),
            Some(
                "Deleting agent-x and spike loses 4 commits that are on no remote and no branch \
                 that stays."
            )
        );
        let body = planned.body.unwrap();
        assert_eq!(
            body["branches"]["current"]["branches"][0]["name"],
            "agent-x"
        );
        assert_eq!(body["branches"]["others"]["branches"][0]["name"], "spike");
    }

    /// After a switch onto `drifted`, `--delete-branch` alone leaves the
    /// second box at its default: ticked, since dux made `agent-x`.
    #[test]
    fn delete_branch_alone_after_a_switch_also_answers_the_second_box() {
        let planned = plan(
            r#"{"current_name":"drifted","current":{"name":"drifted","oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":"after"},
               "current_unpushed":{"count":1,"has_remote_refs":true},
               "others":[{"name":"agent-x","oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","origin":"dux"}],
               "others_unpushed":{"count":0,"has_remote_refs":true},
               "both_unpushed":{"count":4,"has_remote_refs":true},"protected":[]}"#,
            Some(true),
            None,
        );
        assert!(
            planned
                .question
                .contains(", deleting the other branch it was on: agent-x"),
            "{}",
            planned.question
        );
        assert!(
            planned
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("loses 4 commits")),
            "{:?}",
            planned.notice
        );
        assert_eq!(
            planned.body.unwrap()["branches"]["others"]["branches"][0]["name"],
            "agent-x"
        );
    }

    /// `--keep-other-branches` alone keeps the other branches and leaves the
    /// first box at the default the dialogs show: ticked over the branch dux
    /// made, so the question names it as deleted.
    #[test]
    fn a_keep_flag_alone_answers_the_other_box_at_the_dialogs_default() {
        let planned = plan(
            r#"{"current_name":"agent-x","current":{"name":"agent-x","oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":"dux"},
               "current_unpushed":{"count":3,"has_remote_refs":true},
               "others":[{"name":"spike","oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","origin":"dux"}],
               "others_unpushed":{"count":1,"has_remote_refs":true},
               "both_unpushed":{"count":4,"has_remote_refs":true},"protected":[]}"#,
            None,
            Some(false),
        );
        assert!(
            planned.question.contains(", deleting its branch agent-x")
                && planned
                    .question
                    .contains(", keeping the other branch it was on: spike"),
            "{}",
            planned.question
        );
        assert!(!planned.path.contains("delete_branch="), "{}", planned.path);
        let body = planned.body.unwrap();
        assert_eq!(body["branches"]["current"]["delete"], true);
        assert_eq!(body["branches"]["others"]["delete"], false);
        assert_eq!(body["branches"]["others"]["offered"][0], "spike");
    }

    /// A worktree that stays keeps every branch: a keep flag without
    /// --delete-worktree reads nothing and sends the older answer.
    #[test]
    fn a_keep_flag_with_the_worktree_staying_reads_nothing() {
        let planned = plan_with("{}", Worktrees::Keep, Some(false), None);
        assert!(planned.body.is_none());
        assert!(
            planned.path.ends_with("&delete_branch=false"),
            "{}",
            planned.path
        );
    }

    /// The branch the agent is on could not be read: the first box says so,
    /// and the question says that branch is kept.
    #[test]
    fn a_current_branch_that_could_not_be_read_is_named_as_kept() {
        let planned = plan(
            r#"{"current_name":"agent-x","current":null,"others":[],"protected":[]}"#,
            None,
            Some(false),
        );
        assert!(
            planned.question.ends_with(", keeping its branch agent-x"),
            "{}",
            planned.question
        );
        let body = planned.body.unwrap();
        assert_eq!(body["branches"]["current"]["unchecked"], "failed");
    }

    /// Both boxes ticked: the line before the question counts what deleting
    /// both loses, not either box's own count.
    #[test]
    fn the_notice_counts_what_the_ticked_boxes_lose_together() {
        let planned = plan(
            r#"{"current_name":"drifted","current":{"name":"drifted","oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":"after"},
               "current_unpushed":{"count":1,"has_remote_refs":true},
               "others":[{"name":"born","oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","origin":"dux"}],
               "others_unpushed":{"count":0,"has_remote_refs":true},
               "both_unpushed":{"count":4,"has_remote_refs":true},"protected":[]}"#,
            Some(true),
            Some(true),
        );
        assert_eq!(
            planned.notice.as_deref(),
            Some(
                "Deleting drifted and born loses 4 commits that are on no remote and no branch \
                 that stays."
            )
        );
        let body = planned.body.unwrap();
        assert_eq!(body["branches"]["others"]["offered"][0], "born");
        assert_eq!(
            body["branches"]["current"]["branches"][0]["name"],
            "drifted"
        );
    }
    /// Ticked boxes that lose no commit leave nothing to say before the
    /// question.
    #[test]
    fn the_notice_is_absent_when_the_ticked_boxes_lose_nothing() {
        let planned = plan(
            r#"{"current_name":"drifted","current":{"name":"drifted","oid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","origin":"after"},
               "current_unpushed":{"count":0,"has_remote_refs":true},
               "others":[{"name":"born","oid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","origin":"dux"}],
               "others_unpushed":{"count":0,"has_remote_refs":true},
               "both_unpushed":{"count":0,"has_remote_refs":true},"protected":[]}"#,
            Some(true),
            Some(true),
        );
        assert_eq!(planned.notice, None);
    }
}

#[cfg(test)]
mod pull_request_tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, Seen, private_dir};
    use crate::wire::{PullRequestMove, PullRequestMoveNamed, PullRequestSort};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;

    /// A pull request of the shape the sessions read answers with.
    fn pr(number: u64, owner_repo: &str, state: &str, head: Option<&str>, source: &str) -> Value {
        json!({
            "number": number,
            "state": state,
            "title": format!("PR {number}"),
            "url": format!("https://github.com/{owner_repo}/pull/{number}"),
            "host": "github.com",
            "owner_repo": owner_repo,
            "own_repo": owner_repo == "octo/hello",
            "head_branch": head,
            "found": source != "attached",
            "attached": source != "found",
        })
    }

    fn agents() -> String {
        let shown = [
            pr(418, "octo/hello", "open", Some("store-tab-rows"), "found"),
            pr(77, "acme/infra", "open", Some("infra-fix"), "attached"),
            pr(77, "acme/api", "draft", None, "attached"),
            pr(415, "octo/hello", "merged", Some("older"), "found+attached"),
        ];
        json!([{
            "id": "a1",
            "title": "store-tab-rows",
            "provider": "claude",
            "status": "active",
            "workspace": {
                "kind": "managed",
                "project_id": "p1",
                "branch_name": "store-tab-rows",
                "worktree_path": "/wt/s",
            },
            "pr_sort": "manual",
            "pr": shown[0],
            "prs": shown,
            "prs_hidden": [pr(402, "octo/hello", "closed", Some("spike"), "found")],
        }, {
            "id": "a2",
            "title": "notes",
            "provider": "claude",
            "status": "active",
            "workspace": {
                "kind": "folder",
                "folder_path": "/home/me/notes",
                "folder_label": "~/notes",
            },
        }])
        .to_string()
    }

    const BRANCHES: &str = r#"{"branches":[
          {"name":"store-tab-rows","former_names":["tab-rows","rows"],"current":true,"first_seen_at":"2026-10-02T00:00:00Z","prs":[{"host":"github.com","owner_repo":"octo/hello","number":418}]},
          {"name":"older","former_names":[],"current":false,"first_seen_at":"2026-09-01T00:00:00Z","prs":[{"host":"github.com","owner_repo":"octo/hello","number":415},{"host":"GitHub.com","owner_repo":"Octo/Hello","number":402},{"host":"github.com","owner_repo":"octo/hello","number":399}]}
        ],
        "attached_on_no_branch":[{"host":"github.com","owner_repo":"acme/infra","number":77},{"host":"github.com","owner_repo":"acme/api","number":77}]}"#;

    /// Agent `store-tab-rows` in octo/hello, in manual order: #418 found on
    /// its own branch, acme/infra#77 attached, acme/api#77 attached, #415
    /// found on a branch it was on before and attached too; #402 hidden.
    ///
    /// A stand-in dux holding [`agents`], answering what `answer` says for
    /// every other request.
    struct Dux {
        fake: FakeDux,
        client: Client,
        _lock: crate::lockfile::SingleInstanceLock,
        _dir: tempfile::TempDir,
    }

    fn dux(answer: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Dux {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let sessions = agents();
        let fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            "/api/v1/projects" => {
                Reply::json(200, r#"[{"id":"p1","name":"hello","path":"/src/hello"}]"#)
            }
            "/api/v1/sessions" => Reply::json(200, &sessions),
            "/api/v1/sessions/a1/pull-requests/branches" => Reply::json(200, BRANCHES),
            _ => answer(seen),
        });
        let lock_path = dir.path().join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        Dux {
            fake,
            client,
            _lock: lock,
            _dir: dir,
        }
    }

    fn reads() -> Dux {
        dux(|_| Reply::json(404, "{}"))
    }

    #[test]
    fn show_names_the_shown_pull_request_and_lists_the_shown_and_hidden_ones() {
        let dux = reads();
        let shown = agents_show(&dux.client, "store-tab-rows").unwrap();
        let field = |name: &str| -> Value {
            let line = shown
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}: ")))
                .unwrap_or_else(|| panic!("no {name} in {shown}"));
            serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_string()))
        };
        assert_eq!(
            field("pull_request"),
            "#418 open https://github.com/octo/hello/pull/418"
        );
        assert_eq!(
            field("pull_requests"),
            json!([
                "#418 open store-tab-rows https://github.com/octo/hello/pull/418",
                "acme/infra#77 open attached https://github.com/acme/infra/pull/77",
                "acme/api#77 draft attached https://github.com/acme/api/pull/77",
                "#415 merged older https://github.com/octo/hello/pull/415",
            ])
        );
        assert_eq!(
            field("pull_requests_hidden"),
            json!(["#402 closed spike https://github.com/octo/hello/pull/402"])
        );
    }

    #[test]
    fn ls_json_counts_the_shown_pull_requests_and_names_the_first() {
        let dux = reads();
        let listed = agents_ls(&dux.client, None, AgentColumns::All, Shape::Json).unwrap();
        let agents: Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(agents[0]["pull_requests"], 4);
        assert_eq!(
            agents[0]["pull_request"],
            "#418 open https://github.com/octo/hello/pull/418"
        );
    }

    #[test]
    fn a_reference_names_one_pull_request_or_is_refused() {
        let dux = reads();
        let agent = find_agent(&dux.client, "a1").unwrap();
        let url_of = |query: &str| find_pr(&agent, query).map(|pr| pr.url.clone());

        assert_eq!(
            url_of("https://github.com/acme/api/pull/77").unwrap(),
            "https://github.com/acme/api/pull/77"
        );
        assert_eq!(
            url_of("ACME/Infra#77").unwrap(),
            "https://github.com/acme/infra/pull/77"
        );
        assert_eq!(
            url_of("#402").unwrap(),
            "https://github.com/octo/hello/pull/402"
        );
        assert_eq!(
            url_of("418").unwrap(),
            "https://github.com/octo/hello/pull/418"
        );
        for copied in [
            "https://github.com/acme/api/pull/77/files",
            "https://github.com/acme/api/pull/77/",
            "https://github.com/acme/api/pull/77#issuecomment-1",
            "https://GitHub.com/ACME/Api/pull/77",
        ] {
            assert_eq!(
                url_of(copied).unwrap(),
                "https://github.com/acme/api/pull/77",
                "{copied}"
            );
        }
        let elsewhere = url_of("https://ghe.example/acme/api/pull/77#issuecomment-1").unwrap_err();
        assert_eq!(elsewhere.exit, Exit::Failed);
        assert_eq!(
            elsewhere.message,
            "agent store-tab-rows has no pull request \
             https://ghe.example/acme/api/pull/77#issuecomment-1; dux agents prs ls \
             store-tab-rows lists them"
        );

        let two = url_of("#77").unwrap_err();
        assert_eq!(two.exit, Exit::Usage);
        assert_eq!(
            two.message,
            "2 pull requests on agent store-tab-rows are #77: https://github.com/acme/infra/pull/77, \
             https://github.com/acme/api/pull/77; name one as owner/repo#77 or by its URL"
        );

        let none = url_of("9").unwrap_err();
        assert_eq!(none.exit, Exit::Failed);
        assert_eq!(
            none.message,
            "agent store-tab-rows has no pull request #9; dux agents prs ls store-tab-rows lists \
             them"
        );
    }

    #[test]
    fn ls_lists_the_order_then_the_hidden_ones_and_says_when_it_is_your_own() {
        let dux = reads();
        let table = prs_ls(&dux.client, "store-tab-rows", Shape::Table).unwrap();
        let lines: Vec<&str> = table.lines().collect();
        assert!(
            lines[0].starts_with("POS") && lines[0].ends_with("TITLE"),
            "{table}"
        );
        let cells: Vec<Vec<&str>> = lines[1..6]
            .iter()
            .map(|line| {
                line.split("   ")
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .collect();
        assert_eq!(
            cells[0][..5],
            ["1", "#418", "open", "store-tab-rows", "found"]
        );
        assert_eq!(cells[1][..4], ["2", "acme/infra#77", "open", "attached"]);
        assert_eq!(
            cells[3][..5],
            ["4", "#415", "merged", "older", "found+attached"]
        );
        assert_eq!(
            cells[4][..5],
            ["hidden", "#402", "closed", "spike", "found"]
        );
        assert_eq!(
            lines[6],
            "In your own order (dux agents prs sort store-tab-rows auto goes back)."
        );

        let json: Value =
            serde_json::from_str(&prs_ls(&dux.client, "a1", Shape::Json).unwrap()).unwrap();
        assert_eq!(json[0]["position"], 1);
        assert_eq!(json[0]["shown"], true);
        assert_eq!(json[1]["shown"], false);
        assert_eq!(json[4]["position"], Value::Null);
        assert_eq!(json[4]["hidden"], true);
        assert_eq!(json[2]["state"], "draft");

        assert_eq!(
            prs_ls(&dux.client, "a1", Shape::Ids).unwrap(),
            "https://github.com/octo/hello/pull/418\nhttps://github.com/acme/infra/pull/77\n\
             https://github.com/acme/api/pull/77\nhttps://github.com/octo/hello/pull/415\n\
             https://github.com/octo/hello/pull/402\n"
        );
    }

    #[test]
    fn branches_lists_each_branch_with_its_former_names_then_the_attachments_on_none() {
        let dux = reads();
        let table = prs_branches(&dux.client, "a1", Shape::Table).unwrap();
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4, "{table}");
        assert!(lines[1].starts_with("store-tab-rows   yes"), "{table}");
        assert!(lines[1].contains("tab-rows, rows"), "{table}");
        assert!(
            lines[2].ends_with("#415 merged, #402 closed (hidden), octo/hello#399"),
            "{table}"
        );
        assert!(lines[3].starts_with(' '), "{table}");
        assert!(
            lines[3].ends_with("acme/infra#77 open, acme/api#77 draft"),
            "{table}"
        );
        assert_eq!(
            prs_branches(&dux.client, "a1", Shape::Ids).unwrap(),
            "store-tab-rows\nolder\n"
        );
        let json: Value =
            serde_json::from_str(&prs_branches(&dux.client, "a1", Shape::Json).unwrap()).unwrap();
        assert_eq!(
            json[1]["pull_requests"][2],
            json!({
                "host": "github.com",
                "owner_repo": "octo/hello",
                "number": 399,
                "state": null,
                "hidden": null,
                "url": null,
            })
        );
    }

    #[test]
    fn each_change_plans_its_request_and_its_question() {
        let dux = reads();
        let key = |owner_repo: &str, number: u64| json!({ "host": "github.com", "owner_repo": owner_repo, "number": number });
        let cases: Vec<(Planned, &str, Method, &str, Value)> = vec![
            (
                prs_attach(&dux.client, "a1", "acme/infra#77").unwrap(),
                "Attach pull request acme/infra#77 to agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests",
                json!({ "pr": "acme/infra#77" }),
            ),
            (
                prs_detach(&dux.client, "a1", "acme/infra#77").unwrap(),
                "Detach pull request #77 (acme/infra) from agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests/detach",
                key("acme/infra", 77),
            ),
            (
                prs_hide(&dux.client, "a1", "#418").unwrap(),
                "Hide pull request #418 on agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests/hide",
                key("octo/hello", 418),
            ),
            (
                prs_unhide(&dux.client, "a1", "402").unwrap(),
                "Show pull request #402 again on agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests/show",
                key("octo/hello", 402),
            ),
            (
                prs_sort(&dux.client, "a1", PullRequestSort::Automatic).unwrap(),
                "Put agent store-tab-rows's pull requests in automatic order",
                Method::Put,
                "/api/v1/sessions/a1/pull-requests/sort",
                json!({ "mode": "automatic" }),
            ),
            (
                prs_sort(&dux.client, "a1", PullRequestSort::Manual).unwrap(),
                "Put agent store-tab-rows's pull requests in your own order",
                Method::Put,
                "/api/v1/sessions/a1/pull-requests/sort",
                json!({ "mode": "manual" }),
            ),
            (
                prs_move(
                    &dux.client,
                    "a1",
                    "#415",
                    PullRequestMove::Named(PullRequestMoveNamed::Top),
                )
                .unwrap(),
                "Move pull request #415 to the top on agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests/move",
                json!({ "pr": key("octo/hello", 415), "to": "top" }),
            ),
            (
                prs_move(&dux.client, "a1", "#415", PullRequestMove::Position(2)).unwrap(),
                "Move pull request #415 to position 2 on agent store-tab-rows",
                Method::Post,
                "/api/v1/sessions/a1/pull-requests/move",
                json!({ "pr": key("octo/hello", 415), "to": 2 }),
            ),
        ];
        for (planned, question, method, path, body) in cases {
            assert_eq!(planned.question, question);
            assert_eq!((planned.method, planned.path.as_str()), (method, path));
            assert_eq!(planned.body, Some(body), "{question}");
        }
    }

    #[test]
    fn a_change_that_cannot_apply_is_refused_before_anything_is_asked() {
        let dux = reads();
        let detach = prs_detach(&dux.client, "a1", "#418").unwrap_err();
        assert_eq!(detach.exit, Exit::Usage);
        assert_eq!(
            detach.message,
            "#418 on store-tab-rows was found by dux, not attached; hide it instead (dux agents \
             prs hide store-tab-rows https://github.com/octo/hello/pull/418)"
        );
        let hide = prs_hide(&dux.client, "a1", "acme/api#77").unwrap_err();
        assert_eq!(hide.exit, Exit::Usage);
        assert_eq!(
            hide.message,
            "acme/api#77 on store-tab-rows was attached, not found by dux; detach it instead (dux \
             agents prs detach store-tab-rows https://github.com/acme/api/pull/77)"
        );
        let ambiguous = prs_hide(&dux.client, "a1", "77").unwrap_err();
        assert_eq!(ambiguous.exit, Exit::Usage);

        let hidden = prs_move(
            &dux.client,
            "a1",
            "#402",
            PullRequestMove::Named(PullRequestMoveNamed::Top),
        )
        .unwrap_err();
        assert_eq!(hidden.exit, Exit::Usage);
        assert_eq!(
            hidden.message,
            "#402 is hidden on \"store-tab-rows\"; show it again before moving it."
        );
        for position in [0, 5] {
            let beyond = prs_move(
                &dux.client,
                "a1",
                "#418",
                PullRequestMove::Position(position),
            )
            .unwrap_err();
            assert_eq!(beyond.exit, Exit::Usage);
            assert_eq!(
                beyond.message,
                "\"store-tab-rows\" shows 4 pull requests; a position is 1 to 4."
            );
        }

        let standalone = "Agent \"notes\" is a standalone agent: it runs in a folder you chose \
                          and has no branch of its own, so there is nothing to attach, detach or \
                          track a pull request for. Add its folder as a project if you want dux \
                          to manage branches and worktrees for it.";
        let refusals = [
            prs_ls(&dux.client, "a2", Shape::Table).map(|_| ()),
            prs_branches(&dux.client, "a2", Shape::Json).map(|_| ()),
            prs_attach(&dux.client, "a2", "#1").map(|_| ()),
            prs_detach(&dux.client, "a2", "#1").map(|_| ()),
            prs_hide(&dux.client, "a2", "#1").map(|_| ()),
            prs_unhide(&dux.client, "a2", "#1").map(|_| ()),
            prs_sort(&dux.client, "a2", PullRequestSort::Manual).map(|_| ()),
            prs_move(&dux.client, "a2", "#1", PullRequestMove::Position(1)).map(|_| ()),
        ];
        for refusal in refusals {
            let refusal = refusal.unwrap_err();
            assert_eq!(refusal.exit, Exit::Usage);
            assert_eq!(refusal.message, standalone);
        }
        assert!(dux.fake.seen().iter().all(|seen| seen.method == "GET"));
    }

    #[test]
    fn an_attach_and_a_move_report_the_final_their_operation_ended_with() {
        const ATTACHED: &str = "Attached acme/infra#77 to store-tab-rows. It has 5 pull \
                                requests now; the banner shows #418 because open ones come first.";
        const MOVED: &str = "Moved #415 to position 2 on store-tab-rows and switched to your own \
                             order; choose Automatic under Sort to go back.";
        let dux = dux(|seen| match (seen.method.as_str(), seen.path.as_str()) {
            ("POST", "/api/v1/sessions/a1/pull-requests?operation=1") => Reply::json(
                202,
                r#"{"id":"op-1","kind":"pr.attach","state":"running","message":"","created":[],"removed":[],"parts":[]}"#,
            ),
            ("GET", path) if path.starts_with("/api/v1/operations/op-1") => Reply::json(
                200,
                &format!(
                    r#"{{"id":"op-1","kind":"pr.attach","state":"succeeded","message":"{ATTACHED}","created":[],"removed":[],"parts":[]}}"#
                ),
            ),
            ("POST", "/api/v1/sessions/a1/pull-requests/move?operation=1") => Reply::json(
                202,
                &format!(
                    r#"{{"id":"op-2","kind":"pr.move","state":"succeeded","message":"{MOVED}","created":[],"removed":[],"parts":[]}}"#
                ),
            ),
            _ => Reply::json(404, "{}"),
        });
        let attach = prs_attach(&dux.client, "a1", "acme/infra#77").unwrap();
        assert_eq!(
            perform(&dux.client, &attach, Some(Duration::from_secs(5))).unwrap(),
            format!("{ATTACHED}\n")
        );
        let moved = prs_move(&dux.client, "a1", "#415", PullRequestMove::Position(2)).unwrap();
        assert_eq!(
            perform(&dux.client, &moved, Some(Duration::from_secs(5))).unwrap(),
            format!("{MOVED}\n")
        );
    }
}
