//! `dux projects`, `agents`, `agents tabs`, `projects worktrees` and
//! `terminals`: the resources kept in `sessions.sqlite3` and the running
//! engine, so every one of them goes through a running dux.
//!
//! A read builds its list from the API's own reads, in their order. A change
//! is planned first (the resource it names is looked up, and the question to
//! ask is worded), then asked about by the caller, then sent and waited for
//! by [`perform`].

use std::collections::BTreeMap;
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
            Workspace::Folder { folder_path, .. } => Path::new(folder_path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| folder_path.clone()),
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

fn yes_or_blank(flag: bool) -> String {
    if flag { "yes" } else { "" }.to_string()
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// `dux projects ls`.
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
    let flag = |entry: &Value, field: &str| {
        yes_or_blank(entry.get(field).and_then(Value::as_bool).unwrap_or(false))
    };
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

/// `dux agents ls`, of one project when `project` names one. `worktrees`
/// prints only each agent's id and the folder it works in.
pub fn agents_ls(
    client: &Client,
    project: Option<&str>,
    worktrees: bool,
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
    if worktrees {
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
                    .map_or(project_id.clone(), |name| name.to_string()),
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
                }),
            }
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec![
                "ID", "NAME", "PROJECT", "WORKTREE", "PROVIDER", "STATE", "TABS", "REMOTE",
            ],
            rows,
        },
        shape,
    ))
}

/// `dux agents show <a>`.
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
        "pull_request": agent.pr.as_ref().map(|pr| format!("#{} {} {}", pr.number, pr.state, pr.url)),
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

/// A change worked out and not sent yet: what to ask before it, and the
/// request that makes it.
pub struct Planned {
    /// The question asked before it goes ahead, naming what it changes.
    pub question: String,
    method: Method,
    path: String,
    body: Option<Value>,
}

impl Planned {
    fn new(question: String, method: Method, path: String, body: Option<Value>) -> Self {
        Self {
            question,
            method,
            path,
            body,
        }
    }

    /// Go ahead even though somebody else is attached to what this ends.
    fn forced(mut self, force: bool) -> Self {
        if force {
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
pub fn perform(
    client: &Client,
    planned: Planned,
    wait: Option<Duration>,
) -> Result<String, CliError> {
    let deadline = wait.map(wait::deadline_after);
    let record = match client.try_change(planned.method, &planned.path, planned.body.clone(), None)
    {
        Ok(record) => record,
        Err(refused) => match deadline {
            Some(deadline) if refused.error.message == CREATE_IN_FLIGHT_REFUSAL => {
                retry_after_other_create(client, &planned, refused, deadline)?
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
        eprintln!("Another agent was being created, so this one waited for it.");
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
    eprintln!("Another agent was being created (operation {other}), so this one waited for it.");
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
    Ok(lines.into_iter().map(|line| format!("{line}\n")).collect())
}

/// `dux projects add <path>`.
pub fn projects_add(
    client: &Client,
    path: &str,
    name: Option<&str>,
    checkout_default: bool,
    init: bool,
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
            "checkout_default": checkout_default,
            "init_repo": init,
        })),
    ))
}

/// `dux projects rm <p>`.
pub fn projects_rm(
    client: &Client,
    query: &str,
    delete_worktrees: bool,
    force: bool,
) -> Result<Planned, CliError> {
    let projects = projects(client)?;
    let project = find_project(&projects, query)?;
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
    .forced(force))
}

/// How `dux agents add` makes its agent.
pub enum NewAgent {
    /// A new branch and worktree in a project.
    Branch {
        project: String,
        existing_branch: bool,
        copy_uncommitted: bool,
    },
    /// From a pull request of a project.
    PullRequest { project: String, reference: String },
    /// From a worktree a project already has.
    Worktree { project: String, path: String },
    /// A copy of another agent's worktree.
    Fork { agent: String },
    /// In a folder of the user's own, with no project.
    Standalone {
        folder: String,
        provider: Option<String>,
    },
}

/// `dux agents add`.
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
        NewAgent::PullRequest { project, reference } => {
            let (id, project) = project_id(&project)?;
            (
                format!("Create {called} from pull request {reference} in project {project}"),
                json!({ "kind": "from_pr", "project_id": id, "pr": reference, "name": name }),
            )
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

/// `dux agents rm <a>`. `delete_branch` is the dialogs' answer: `None`
/// leaves it to how the branch came to be.
pub fn agents_rm(
    client: &Client,
    query: &str,
    delete_worktree: bool,
    delete_branch: Option<bool>,
    force: bool,
) -> Result<Planned, CliError> {
    let agent = find_agent(client, query)?;
    let mut question = format!("Delete agent {}", agent.name());
    let mut path = format!(
        "/api/v1/sessions/{}?delete_worktree={delete_worktree}",
        segment(&agent.id)
    );
    if delete_worktree {
        question.push_str(&format!(" and its worktree {}", agent.directory()));
    }
    match delete_branch {
        Some(true) => question.push_str(", deleting its branch"),
        Some(false) => question.push_str(", keeping its branch"),
        None => {}
    }
    if let Some(delete) = delete_branch {
        path.push_str(&format!("&delete_branch={delete}"));
    }
    Ok(Planned::new(question, Method::Delete, path, None).forced(force))
}

/// `dux agents stop <a>`.
pub fn agents_stop(client: &Client, query: &str, force: bool) -> Result<Planned, CliError> {
    let agent = find_agent(client, query)?;
    Ok(Planned::new(
        format!("Stop everything agent {} runs", agent.name()),
        Method::Post,
        format!("/api/v1/sessions/{}/kill", segment(&agent.id)),
        None,
    )
    .forced(force))
}

/// `dux agents start <a>`.
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
pub fn tabs_rm(client: &Client, agent: &str, tab: &str, force: bool) -> Result<Planned, CliError> {
    let (agent, label, path) = tab_address(client, agent, tab)?;
    Ok(Planned::new(
        format!("Close the {label} tab of agent {agent}"),
        Method::Delete,
        path,
        None,
    )
    .forced(force))
}

/// `dux agents tabs start <a> <tab>`.
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
pub fn tabs_stop(
    client: &Client,
    agent: &str,
    tab: &str,
    force: bool,
) -> Result<Planned, CliError> {
    let (agent, label, path) = tab_address(client, agent, tab)?;
    Ok(Planned::new(
        format!("Stop the {label} tab of agent {agent}"),
        Method::Post,
        format!("{path}/stop"),
        None,
    )
    .forced(force))
}

/// Who a new terminal belongs to.
pub enum TerminalFor {
    Agent(String),
    Project(String),
    Standalone,
}

/// `dux terminals add`.
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
pub fn terminals_rm(client: &Client, query: &str, force: bool) -> Result<Planned, CliError> {
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
    .forced(force))
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
    fn a_create_refused_for_another_create_waits_for_it_and_tries_again() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED]));
        let out = perform(&client, add(&client), Some(Duration::from_secs(5))).unwrap();
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
        let out = perform(&client, add(&client), Some(Duration::from_secs(10))).unwrap();
        assert_eq!(out, "Created agent second.\ns2\n");
        assert_eq!(posts(&fake), 3);
    }

    #[test]
    fn with_no_wait_a_create_refused_for_another_create_is_not_retried() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED]));
        let error = perform(&client, add(&client), None).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, SENTENCE);
        assert_eq!(posts(&fake), 1);
    }

    #[test]
    fn a_create_refused_again_after_waiting_fails_with_the_refusal() {
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![NAMED, NAMED]));
        let error = perform(&client, add(&client), Some(Duration::from_secs(5))).unwrap_err();
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
        assert!(perform(&client, add(&client), Some(Duration::from_secs(5))).is_ok());
        assert_eq!(posts(&fake), 2);

        // A server error is read again within the time, then the answer counts.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(500, "boom"), (200, OTHER_DONE)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        assert!(perform(&client, add(&client), Some(Duration::from_secs(5))).is_ok());
        assert_eq!(posts(&fake), 2);

        // Any other refusal of the read is the command's error, and nothing is sent again.
        let dir = private_dir();
        let mut script = Script::new(vec![NAMED]);
        script.other = vec![(400, r#"{"error":"bad request"}"#)];
        let (fake, _lock, client) = dux_running(dir.path(), script);
        let error = perform(&client, add(&client), Some(Duration::from_secs(5))).unwrap_err();
        assert_eq!(error.exit, Exit::Failed);
        assert_eq!(error.message, "bad request");
        assert_eq!(posts(&fake), 1);
    }

    #[test]
    fn waiting_for_the_other_creation_comes_out_of_the_one_time_the_command_has() {
        // The other creation takes about 0.75 s of a 1 s wait; the new agent then
        // never finishes. Its wait gets the 0.25 s left, so the command ends near
        // 1 s in all, not near 1.75 s.
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
        let error = perform(&client, add(&client), Some(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.exit, Exit::Unknown);
        assert!(
            started.elapsed() < Duration::from_millis(1400),
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
        let error = perform(&client, add(&client), Some(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, SENTENCE);
        assert_eq!(posts(&fake), 1);

        // The same with no operation named: it gives up with the refusal.
        let dir = private_dir();
        let (fake, _lock, client) = dux_running(dir.path(), Script::new(vec![SENTENCE; 100]));
        let started = Instant::now();
        let error = perform(&client, add(&client), Some(Duration::from_secs(1))).unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert!(started.elapsed() < Duration::from_millis(1400));
        assert!(posts(&fake) <= 3, "{} creates", posts(&fake));
    }
}
