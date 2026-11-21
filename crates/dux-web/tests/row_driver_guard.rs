//! The server refuses a request that would change a row of the agents list
//! another device drives, whoever sends it: `423 Locked` naming the driver.
//! The driver itself, anybody while nobody drives, and a request that says to
//! go ahead over everybody (`force_connected=true`) are let through. A request
//! is the driver's when it names the page id the driving terminal sockets
//! opened with, whatever that page's events socket is doing.

use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::time::Duration;

use dux_core::config::{DuxPaths, ProjectConfig, ProviderCommandConfig};
use dux_core::storage::SessionStore;
use dux_web::bootstrap::bootstrap_engine;
use dux_web::engine_actor::spawn_engine_thread;
use dux_web::server::router;
use futures_util::{SinkExt, StreamExt};
use reqwest::Method;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
const SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                      (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/// A server with project `p1` and agent `s1`, whose working copy is a git
/// repository with one committed file, `f.txt`, changed on disk.
async fn boot() -> (SocketAddr, dux_core::test_scratch::ScratchDir) {
    let tmp = dux_core::test_scratch::ScratchDir::new();
    let root = tmp.path().to_path_buf();
    let run = |args: &[&str]| {
        let ok = dux_core::test_git::fixture_git()
            .args(args)
            .current_dir(&root)
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "t"]);
    std::fs::write(root.join("f.txt"), "one\n").expect("write the file");
    run(&["add", "f.txt"]);
    run(&["commit", "-q", "-m", "init"]);
    std::fs::write(root.join("f.txt"), "two\n").expect("change the file");

    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).expect("create the test directory");
    {
        let store = SessionStore::open(&paths.sessions_db_path).expect("open the session store");
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: root.to_string_lossy().into_owned(),
                name: Some("p1".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: BTreeMap::new(),
            })
            .expect("save the test project");
        let now = chrono::Utc::now();
        store
            .create_session(&dux_core::model::AgentSession {
                id: "s1".to_string(),
                slot_tab_id: "s1-slot".to_string(),
                provider: dux_core::model::ProviderKind::new("claude"),
                title: None,
                started_providers: Vec::new(),
                desired_running: true,
                auto_reopen_enabled: false,
                status: dux_core::model::SessionStatus::Detached,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
                workspace: dux_core::model::AgentWorkspace::Managed(
                    dux_core::model::ManagedWorkspace {
                        project_id: "p1".to_string(),
                        project_path: None,
                        source_branch: "main".to_string(),
                        branch_name: "feat".to_string(),
                        initial_branch: "feat".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_path: root.to_string_lossy().into_owned(),
                    },
                ),
            })
            .expect("save the test agent");
    }
    let mut engine = bootstrap_engine(&paths).expect("bootstrap the test engine");
    dux_core::test_provider::defuse_config(&mut engine.config);
    engine.config.providers.commands.insert(
        "claude".to_string(),
        ProviderCommandConfig {
            command: "cat".to_string(),
            args: vec![],
            resume_args: None,
            ..Default::default()
        },
    );
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let (handle, _join) = spawn_engine_thread(engine);
    let app = router(handle);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener.local_addr().expect("read the bound address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve the test router");
    });
    (addr, tmp)
}

async fn connect(addr: SocketAddr, path: &str, user_agent: &str) -> (ClientWs, serde_json::Value) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{addr}{path}")
        .into_client_request()
        .expect("build the websocket request");
    request.headers_mut().insert(
        "user-agent",
        user_agent.parse().expect("a valid user agent header"),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("open the websocket");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(frame) = serde_json::from_str::<serde_json::Value>(&text)
            && frame["event"] == "connected"
        {
            return (ws, frame);
        }
    }
    panic!("{path} never sent its connected frame");
}

/// A page id no other page of this test run has.
fn mint_page_id() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{:032x}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// One browser page: the id its terminal sockets name, its events socket and
/// the terminal sockets it has open.
struct Page {
    id: String,
    events: String,
    user_agent: &'static str,
    events_socket: Option<ClientWs>,
    sockets: Vec<ClientWs>,
}

impl Page {
    async fn open(addr: SocketAddr, user_agent: &'static str) -> Self {
        let (socket, hello) = connect(addr, "/ws/events", user_agent).await;
        Self {
            id: mint_page_id(),
            events: hello["id"].as_str().expect("an events id").to_string(),
            user_agent,
            events_socket: Some(socket),
            sockets: Vec::new(),
        }
    }

    /// Open the terminal socket at `path` as part of this page.
    async fn show(&mut self, addr: SocketAddr, path: &str) -> usize {
        let (socket, _) = connect(
            addr,
            &format!("{path}?events={}&page={}", self.events, self.id),
            self.user_agent,
        )
        .await;
        self.sockets.push(socket);
        self.sockets.len() - 1
    }

    /// Open the terminal socket at `path` before this page's events socket has
    /// told it its id, so the socket names the page alone.
    async fn show_before_its_events_id(&mut self, addr: SocketAddr, path: &str) -> usize {
        let (socket, _) = connect(addr, &format!("{path}?page={}", self.id), self.user_agent).await;
        self.sockets.push(socket);
        self.sockets.len() - 1
    }

    /// Type into socket `index`, which claims a row nobody drives.
    async fn type_into(&mut self, index: usize) {
        self.sockets[index]
            .send(Message::Binary(b"hello\n".to_vec().into()))
            .await
            .expect("type");
    }

    async fn take_over(&mut self, index: usize) {
        self.sockets[index]
            .send(Message::Text(
                r#"{"rows":30,"cols":100,"takeover":true}"#.into(),
            ))
            .await
            .expect("take over");
    }
}

/// Wait until the connections listing says `events` drives `count` of what it
/// has open.
async fn wait_until_driving(addr: SocketAddr, events: &str, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let listed: serde_json::Value =
            reqwest::get(format!("http://{addr}/api/v1/server/connections"))
                .await
                .expect("GET connections")
                .json()
                .await
                .expect("a JSON listing");
        let driving = listed
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| entry["id"] == events)
            .flat_map(|entry| entry["attachments"].as_array().cloned().unwrap_or_default())
            .filter(|attachment| attachment["driving"] == true)
            .count();
        if driving == count {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{events} drives {driving} of its panes, not {count}: {listed}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Who a request is sent as.
#[derive(Clone, Copy)]
enum As<'a> {
    /// A client with no connection: the command line, a direct call.
    Nobody,
    /// A browser page naming this page id, or these, comma separated.
    Page(&'a str),
    /// A request naming this events id and no page.
    Events(&'a str),
    /// A client with no connection saying to go ahead over everybody.
    Forcing,
}

async fn send(
    addr: SocketAddr,
    method: &Method,
    path: &str,
    body: Option<&serde_json::Value>,
    sender: As<'_>,
) -> (u16, String) {
    let force = match (matches!(sender, As::Forcing), path.contains('?')) {
        (false, _) => "",
        (true, false) => "?force_connected=true",
        (true, true) => "&force_connected=true",
    };
    let mut request =
        reqwest::Client::new().request(method.clone(), format!("http://{addr}{path}{force}"));
    match sender {
        As::Page(page) => request = request.header("x-dux-page", page),
        As::Events(events) => request = request.header("x-connection-id", events),
        As::Nobody | As::Forcing => {}
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.expect("send the request");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

async fn create_terminal(addr: SocketAddr, path: &str) -> String {
    let (status, body) = send(addr, &Method::POST, path, None, As::Nobody).await;
    assert_eq!(status, 201, "{path}: {body}");
    let created: serde_json::Value = serde_json::from_str(&body).expect("a created terminal");
    created["terminal_id"]
        .as_str()
        .expect("a terminal id")
        .to_string()
}

fn assert_refused(what: &str, answer: &(u16, String)) {
    let (status, body) = answer;
    assert_eq!(*status, 423, "{what}: {body}");
    assert!(
        body.contains("Firefox on Linux"),
        "{what} names the driver: {body}"
    );
    assert!(body.contains("did not make the change"), "{what}: {body}");
}

/// Whether a handler changes a row a device may be driving.
enum Decision {
    /// Refused while another device drives the row.
    Gated,
    /// Open to everybody, for this reason.
    Open(&'static str),
}

const READ: Decision = Decision::Open("a read sent as a POST");
const CONFIG: Decision = Decision::Open("config and preferences");
const PROJECT: Decision = Decision::Open("project-level settings, git and folders");
const PULL_REQUESTS: Decision = Decision::Open("pull-request bookkeeping");
const NEW_ROW: Decision = Decision::Open("makes a new row nobody drives yet");
const LIST_ORDER: Decision = Decision::Open("where rows sit in the list");
const SIGN_IN: Decision = Decision::Open("signing in and out");

/// Every handler a route registers under a changing method, with its decision.
/// A handler missing here fails `every_changing_route_has_a_decision`.
const DECISIONS: &[(&str, Decision)] = &[
    ("create_tab", Decision::Gated),
    ("reorder_tabs", Decision::Gated),
    ("delete_tab", Decision::Gated),
    ("retarget_tab", Decision::Gated),
    ("start_tab", Decision::Gated),
    ("stop_tab", Decision::Gated),
    ("create_terminal", Decision::Gated),
    ("delete_terminal", Decision::Gated),
    ("delete_project_terminal", Decision::Gated),
    ("delete_standalone_terminal", Decision::Gated),
    ("reorder_terminals", Decision::Gated),
    ("patch_session", Decision::Gated),
    ("delete_session", Decision::Gated),
    ("reconnect_session", Decision::Gated),
    ("kill_session", Decision::Gated),
    ("recreate_working_copy", Decision::Gated),
    ("rerun_startup_command", Decision::Gated),
    ("stage", Decision::Gated),
    ("unstage", Decision::Gated),
    ("stage_files", Decision::Gated),
    ("unstage_files", Decision::Gated),
    ("discard", Decision::Gated),
    ("commit", Decision::Gated),
    ("push", Decision::Gated),
    ("pull", Decision::Gated),
    ("write_file", Decision::Gated),
    ("create_file", Decision::Gated),
    ("create_dir", Decision::Gated),
    ("rename_entry", Decision::Gated),
    ("delete_entry", Decision::Gated),
    ("upload_dropped_file", Decision::Gated),
    ("list_files", READ),
    ("list_tree", READ),
    ("read_file", READ),
    ("diff_contents", READ),
    ("entry_info", READ),
    ("writable", READ),
    ("refresh_changes", READ),
    ("resolve_pull_request_reference", READ),
    ("check_pull_request_clone_destination", READ),
    (
        "open_in_editor",
        Decision::Open("opens a file in an editor on the server's machine; writes nothing"),
    ),
    (
        "set_focused_tab_route",
        Decision::Open("which tab a device has on top, which selecting a tab sets"),
    ),
    ("create_session", NEW_ROW),
    ("create_project_terminal", NEW_ROW),
    ("create_standalone_terminal", NEW_ROW),
    ("reorder_agents", LIST_ORDER),
    ("reorder_projects", LIST_ORDER),
    ("place_terminal_in_list", LIST_ORDER),
    ("attach_pull_request", PULL_REQUESTS),
    ("detach_pull_request", PULL_REQUESTS),
    ("hide_pull_request", PULL_REQUESTS),
    ("show_pull_request", PULL_REQUESTS),
    ("set_pull_request_sort", PULL_REQUESTS),
    ("move_pull_request", PULL_REQUESTS),
    ("reorder_pull_requests", PULL_REQUESTS),
    ("add_project", PROJECT),
    ("patch_project", PROJECT),
    ("pull_project", PROJECT),
    ("checkout_default", PROJECT),
    ("change_base_branch", PROJECT),
    (
        "remove_project",
        Decision::Open("ends rows, which the attachment guard judges against everybody attached"),
    ),
    ("delete_worktree", PROJECT),
    ("delete_leftover_folder", PROJECT),
    (
        "mkdir",
        Decision::Open("makes a folder in the folder browser, in no row"),
    ),
    ("update_macros", CONFIG),
    ("set_macro", CONFIG),
    ("remove_macro", CONFIG),
    ("persist_global_env", CONFIG),
    ("set_global_env_var", CONFIG),
    ("remove_global_env_var", CONFIG),
    ("set_changes_pane", CONFIG),
    ("reload_config", CONFIG),
    ("write_raw_config", CONFIG),
    ("toggle_randomized_pet_name_default", CONFIG),
    ("toggle_pr_banner_position", CONFIG),
    ("set_agent_sort", CONFIG),
    ("toggle_github_integration", CONFIG),
    ("recheck_github", CONFIG),
    ("toggle_copy_on_select", CONFIG),
    ("toggle_always_show_tab_strip", CONFIG),
    ("set_instance_identity", CONFIG),
    ("set_settings", CONFIG),
    ("set_tailscale_mode", CONFIG),
    ("dismiss_first_load", CONFIG),
    ("dismiss_no_auth_warning", CONFIG),
    ("login", SIGN_IN),
    ("logout", SIGN_IN),
    ("cli_login", SIGN_IN),
    ("cli_logout", SIGN_IN),
    ("change_password", SIGN_IN),
];

/// The handlers the server's sources register under `post`, `put`, `patch` or
/// `delete`, test modules left out.
fn changing_handlers() -> BTreeSet<String> {
    fn sources(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read the source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut handlers = BTreeSet::new();
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read a source file");
        let text = text.split("#[cfg(test)]").next().unwrap_or_default();
        for method in ["post(", "put(", "patch(", "delete("] {
            for (at, _) in text.match_indices(method) {
                // `.delete(` chained onto another method's `)` is a route;
                // on anything else it is some other method of that name.
                let before = &text[..at];
                let called_on_something = match before.strip_suffix('.') {
                    Some(receiver) => !receiver.trim_end().ends_with(')'),
                    None => before
                        .chars()
                        .next_back()
                        .is_some_and(|c| c == '_' || c.is_alphanumeric()),
                };
                let name: String = text[at + method.len()..]
                    .trim_start()
                    .chars()
                    .take_while(|c| *c == '_' || c.is_alphanumeric())
                    .collect();
                if !called_on_something && !name.is_empty() {
                    handlers.insert(name);
                }
            }
        }
    }
    handlers
}

fn gated_handlers() -> BTreeSet<String> {
    DECISIONS
        .iter()
        .filter(|(_, decision)| matches!(decision, Decision::Gated))
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// A route that changes something is a place the row question can be left
/// out, so each one is decided by name: adding a route under a changing method
/// fails here until it is listed as gated or as open with its reason.
#[test]
fn every_changing_route_has_a_decision() {
    let decided: BTreeSet<String> = DECISIONS
        .iter()
        .map(|(name, decision)| {
            if let Decision::Open(reason) = decision {
                assert!(!reason.is_empty(), "{name} is open with no reason");
            }
            (*name).to_string()
        })
        .collect();
    assert_eq!(decided.len(), DECISIONS.len(), "a handler is listed twice");
    let registered = changing_handlers();
    let undecided: Vec<&String> = registered.difference(&decided).collect();
    assert!(
        undecided.is_empty(),
        "these handlers change something and nobody has said whether they change a row a \
         device may be driving; list each in DECISIONS: {undecided:?}"
    );
    let gone: Vec<&String> = decided.difference(&registered).collect();
    assert!(
        gone.is_empty(),
        "DECISIONS lists handlers no route has: {gone:?}"
    );
}

const AGENT: &str = "/api/v1/sessions/s1";

/// The terminals the walk has running beside agent `s1`.
struct Terminals {
    docked: String,
    docked_too: String,
    of_project: String,
    standalone: String,
}

/// One request to a gated handler.
struct Ask {
    handler: &'static str,
    method: Method,
    path: String,
    body: Option<serde_json::Value>,
}

fn ask(handler: &'static str, method: Method, path: String, body: serde_json::Value) -> Ask {
    Ask {
        handler,
        method,
        path,
        body: (!body.is_null()).then_some(body),
    }
}

fn post(handler: &'static str, path: String, body: serde_json::Value) -> Ask {
    ask(handler, Method::POST, path, body)
}

fn requests_for_the_agent_and_its_parts(terminals: &Terminals) -> Vec<Ask> {
    let Terminals {
        docked,
        docked_too,
        of_project,
        standalone,
    } = terminals;
    let none = serde_json::Value::Null;
    let tab = format!("{AGENT}/tabs/s1-slot");
    let order = json!({ "terminal_ids": [docked_too, docked, of_project, standalone] });
    vec![
        post("create_tab", format!("{AGENT}/tabs"), none.clone()),
        post(
            "reorder_tabs",
            format!("{AGENT}/tabs/reorder"),
            json!({ "tab_ids": ["s1-slot"] }),
        ),
        ask("delete_tab", Method::DELETE, tab.clone(), none.clone()),
        ask(
            "retarget_tab",
            Method::PATCH,
            tab.clone(),
            json!({ "provider": "claude" }),
        ),
        post("start_tab", format!("{tab}/start"), none.clone()),
        post("stop_tab", format!("{tab}/stop"), none.clone()),
        post(
            "create_terminal",
            format!("{AGENT}/terminals"),
            none.clone(),
        ),
        ask(
            "delete_terminal",
            Method::DELETE,
            format!("{AGENT}/terminals/{docked}"),
            none.clone(),
        ),
        ask(
            "delete_project_terminal",
            Method::DELETE,
            format!("/api/v1/projects/p1/terminals/{of_project}"),
            none.clone(),
        ),
        ask(
            "delete_standalone_terminal",
            Method::DELETE,
            format!("/api/v1/terminals/{standalone}"),
            none.clone(),
        ),
        post(
            "reorder_terminals",
            "/api/v1/terminals/reorder".into(),
            order,
        ),
        ask(
            "patch_session",
            Method::PATCH,
            AGENT.into(),
            json!({ "title": "renamed" }),
        ),
        ask(
            "patch_session",
            Method::PATCH,
            AGENT.into(),
            json!({ "provider": "claude" }),
        ),
        ask(
            "patch_session",
            Method::PATCH,
            AGENT.into(),
            json!({ "auto_reopen": true }),
        ),
        ask("delete_session", Method::DELETE, AGENT.into(), none.clone()),
        post(
            "reconnect_session",
            format!("{AGENT}/reconnect"),
            none.clone(),
        ),
        post("kill_session", format!("{AGENT}/kill"), none.clone()),
        post(
            "recreate_working_copy",
            format!("{AGENT}/recreate-working-copy"),
            none.clone(),
        ),
        post(
            "rerun_startup_command",
            format!("{AGENT}/rerun-startup-command"),
            none,
        ),
    ]
}

fn requests_for_files(terminals: &Terminals) -> Vec<Ask> {
    let file = json!({ "path": "f.txt" });
    let files = json!({ "paths": ["f.txt"] });
    let new_file = json!({ "path": "new.txt" });
    let of_project = &terminals.of_project;
    let standalone = &terminals.standalone;
    vec![
        post("stage", format!("{AGENT}/git/stage"), file.clone()),
        post("unstage", format!("{AGENT}/git/unstage"), file.clone()),
        post(
            "stage_files",
            format!("{AGENT}/git/stage-files"),
            files.clone(),
        ),
        post("unstage_files", format!("{AGENT}/git/unstage-files"), files),
        post("discard", format!("{AGENT}/git/discard"), file.clone()),
        post(
            "commit",
            format!("{AGENT}/git/commit"),
            json!({ "message": "a commit" }),
        ),
        post("push", format!("{AGENT}/git/push"), json!({})),
        post("pull", format!("{AGENT}/git/pull"), json!({})),
        post(
            "write_file",
            format!("{AGENT}/files/write"),
            json!({ "path": "f.txt", "content": "three\n" }),
        ),
        post(
            "create_file",
            format!("{AGENT}/files/create-file"),
            new_file.clone(),
        ),
        post(
            "create_dir",
            format!("{AGENT}/files/create-dir"),
            json!({ "path": "new-dir" }),
        ),
        post(
            "rename_entry",
            format!("{AGENT}/files/rename"),
            json!({ "from": "f.txt", "to": "g.txt" }),
        ),
        post("delete_entry", format!("{AGENT}/files/delete"), file),
        // An editor rooted at a project or standalone terminal is that terminal's row.
        post(
            "create_file",
            format!("/api/v1/projects/p1/terminals/{of_project}/files/create-file"),
            new_file,
        ),
        post(
            "create_file",
            format!("/api/v1/terminals/{standalone}/files/create-file"),
            json!({ "path": "no-such-folder/new.txt" }),
        ),
        post(
            "upload_dropped_file",
            "/api/v1/file-drop?pty=s1&filename=dropped.txt".into(),
            json!("dropped"),
        ),
        post(
            "upload_dropped_file",
            "/api/v1/file-drop?pty=s1&filename=dropped.txt&dir=".into(),
            json!("dropped"),
        ),
    ]
}

/// With one page driving the agent, a project terminal and a standalone
/// terminal, every gated route refuses a client with no connection and a
/// second page that only watches, naming the driver, and nothing is changed.
#[tokio::test]
async fn every_gated_route_refuses_whoever_does_not_drive_the_row() {
    let (addr, tmp) = boot().await;
    let docked = create_terminal(addr, "/api/v1/sessions/s1/terminals").await;
    let docked_too = create_terminal(addr, "/api/v1/sessions/s1/terminals").await;
    let of_project = create_terminal(addr, "/api/v1/projects/p1/terminals").await;
    let standalone = create_terminal(addr, "/api/v1/terminals").await;

    let mut driver = Page::open(addr, FIREFOX).await;
    for path in [
        "/ws/sessions/s1/pty".to_string(),
        format!("/ws/projects/p1/terminals/{of_project}/pty"),
        format!("/ws/terminals/{standalone}/pty"),
    ] {
        let socket = driver.show(addr, &path).await;
        driver.type_into(socket).await;
    }
    wait_until_driving(addr, &driver.events, 3).await;
    let mut watcher = Page::open(addr, SAFARI).await;
    watcher.show(addr, "/ws/sessions/s1/pty").await;

    let terminals = Terminals {
        docked,
        docked_too,
        of_project,
        standalone,
    };
    let mut requests = requests_for_the_agent_and_its_parts(&terminals);
    requests.extend(requests_for_files(&terminals));

    let walked: BTreeSet<String> = requests
        .iter()
        .map(|request| request.handler.to_string())
        .collect();
    assert_eq!(
        walked,
        gated_handlers(),
        "every gated handler is sent a request here"
    );

    for Ask {
        handler,
        method,
        path,
        body,
    } in &requests
    {
        for (sender, who) in [
            (As::Nobody, "a client with no connection"),
            (As::Page(&watcher.id), "a page that only watches"),
        ] {
            let answer = send(addr, method, path, body.as_ref(), sender).await;
            assert_refused(&format!("{handler} ({method} {path}) from {who}"), &answer);
        }
    }

    let agent = AGENT;
    let root = tmp.path();
    assert_eq!(
        std::fs::read_to_string(root.join("f.txt")).expect("the file is still there"),
        "two\n"
    );
    for untouched in ["new.txt", "new-dir", "g.txt", ".dux-uploads"] {
        assert!(!root.join(untouched).exists(), "{untouched} was made");
    }
    let (status, body) = send(addr, &Method::GET, agent, None, As::Nobody).await;
    assert_eq!(status, 200, "reads stay open: {body}");
    let session: serde_json::Value = serde_json::from_str(&body).expect("the agent");
    assert_eq!(session["auto_reopen_enabled"], false, "{session}");
}

/// The driver's own requests go through, one per way a change reaches the
/// engine, and so does a request that says to go ahead over everybody.
#[tokio::test]
async fn the_driver_and_a_forced_request_are_let_through() {
    let (addr, tmp) = boot().await;
    let mut driver = Page::open(addr, FIREFOX).await;
    let socket = driver.show(addr, "/ws/sessions/s1/pty").await;
    driver.type_into(socket).await;
    wait_until_driving(addr, &driver.events, 1).await;

    let agent = "/api/v1/sessions/s1";
    let reopen = |enabled: bool| Some(json!({ "auto_reopen": enabled }));
    // The page's events socket is away, as it is while it reconnects.
    drop(driver.events_socket.take());
    let as_driver = As::Page(&driver.id);

    // A wire command.
    let refused = send(
        addr,
        &Method::PATCH,
        agent,
        reopen(true).as_ref(),
        As::Nobody,
    )
    .await;
    assert_refused("a setting from a client with no connection", &refused);
    let (status, body) = send(
        addr,
        &Method::PATCH,
        agent,
        reopen(true).as_ref(),
        as_driver,
    )
    .await;
    assert_eq!(status, 200, "the driver's setting: {body}");
    let (status, body) = send(
        addr,
        &Method::PATCH,
        agent,
        reopen(false).as_ref(),
        As::Forcing,
    )
    .await;
    assert_eq!(status, 200, "a forced setting: {body}");

    // A request the engine answers outside the wire commands.
    let terminals = format!("{agent}/terminals");
    let (status, body) = send(addr, &Method::POST, &terminals, None, as_driver).await;
    assert_eq!(status, 201, "the driver's new terminal: {body}");
    let (status, body) = send(addr, &Method::POST, &terminals, None, As::Forcing).await;
    assert_eq!(status, 201, "a forced new terminal: {body}");

    // Changes the web layer makes itself: git, the editor, a dropped file.
    let stage = format!("{agent}/git/stage");
    let file = json!({ "path": "f.txt" });
    let (status, body) = send(addr, &Method::POST, &stage, Some(&file), as_driver).await;
    assert_eq!(status, 200, "the driver's stage: {body}");
    let unstage = format!("{agent}/git/unstage");
    let (status, body) = send(addr, &Method::POST, &unstage, Some(&file), As::Forcing).await;
    assert_eq!(status, 200, "a forced unstage: {body}");
    let create = format!("{agent}/files/create-file");
    let new_file = json!({ "path": "new.txt" });
    let (status, body) = send(addr, &Method::POST, &create, Some(&new_file), as_driver).await;
    assert_eq!(status, 200, "the driver's new file: {body}");
    assert!(tmp.path().join("new.txt").exists());
    let dropped = "/api/v1/file-drop?pty=s1&filename=dropped.txt";
    let (status, body) = send(addr, &Method::POST, dropped, Some(&json!("x")), as_driver).await;
    assert_eq!(status, 200, "the driver's dropped file: {body}");
}

/// The row is open to everybody until somebody drives it, passes to whoever
/// takes it over, and is open again once the driver's page has gone.
#[tokio::test]
async fn the_refusal_follows_who_drives_the_row() {
    let (addr, _tmp) = boot().await;
    let agent = "/api/v1/sessions/s1";
    let reopen = json!({ "auto_reopen": true });
    let ask = |sender| send(addr, &Method::PATCH, agent, Some(&reopen), sender);

    let (status, body) = ask(As::Nobody).await;
    assert_eq!(status, 200, "nobody drives a row nobody has opened: {body}");

    let mut first = Page::open(addr, FIREFOX).await;
    let mut second = Page::open(addr, SAFARI).await;
    let socket = first.show(addr, "/ws/sessions/s1/pty").await;
    first.type_into(socket).await;
    wait_until_driving(addr, &first.events, 1).await;
    let watching = second.show(addr, "/ws/sessions/s1/pty").await;
    let (first_page, second_page) = (first.id.clone(), second.id.clone());
    let second_events = second.events.clone();
    assert_refused(
        "the page that only watches",
        &ask(As::Page(&second_page)).await,
    );

    second.take_over(watching).await;
    wait_until_driving(addr, &second_events, 1).await;
    let (status, body) = ask(As::Page(&second_page)).await;
    assert_eq!(status, 200, "the driver right after taking over: {body}");
    let (status, body) = ask(As::Page(&first_page)).await;
    assert_eq!(status, 423, "the page that lost the row: {body}");
    assert!(body.contains("Safari on iOS"), "{body}");
    // An events id is told to nobody else but proves nothing about driving.
    let (status, body) = ask(As::Events(&second_events)).await;
    assert_eq!(status, 423, "the driver's events id with no page: {body}");

    drop(second);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let (status, body) = ask(As::Nobody).await;
        if status == 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the row is still driven after its driver's page closed: {status} {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Whether a write under the agent's editor root would be let through.
async fn writable(addr: SocketAddr, sender: As<'_>) -> u16 {
    let path = "/api/v1/sessions/s1/files/writable";
    send(addr, &Method::POST, path, None, sender).await.0
}

/// A page whose terminal socket opened before its events socket said its id
/// drives all the same: every way a change reaches the engine goes through for
/// it, and for an editor tab that names it beside a page of its own.
#[tokio::test]
async fn the_driving_page_needs_no_events_connection() {
    let (addr, tmp) = boot().await;
    let mut driver = Page::open(addr, FIREFOX).await;
    let socket = driver
        .show_before_its_events_id(addr, "/ws/sessions/s1/pty")
        .await;
    driver.type_into(socket).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while writable(addr, As::Nobody).await != 423 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the row was never driven"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(writable(addr, As::Page(&driver.id)).await, 204);
    assert_eq!(writable(addr, As::Page(&mint_page_id())).await, 423);

    let agent = "/api/v1/sessions/s1";
    let file = json!({ "path": "f.txt" });
    let editor_tab = format!("{},{}", mint_page_id(), driver.id);
    for (what, sender) in [
        ("the driver", As::Page(&driver.id)),
        ("an editor tab the driver opened", As::Page(&editor_tab)),
    ] {
        let reopen = json!({ "auto_reopen": true });
        let (status, body) = send(addr, &Method::PATCH, agent, Some(&reopen), sender).await;
        assert_eq!(status, 200, "a setting from {what}: {body}");
        let terminals = format!("{agent}/terminals");
        let (status, body) = send(addr, &Method::POST, &terminals, None, sender).await;
        assert_eq!(status, 201, "a new terminal from {what}: {body}");
        let stage = format!("{agent}/git/stage");
        let (status, body) = send(addr, &Method::POST, &stage, Some(&file), sender).await;
        assert_eq!(status, 200, "a stage from {what}: {body}");
        let write = format!("{agent}/files/write");
        let text = json!({ "path": "f.txt", "content": "three\n" });
        let (status, body) = send(addr, &Method::POST, &write, Some(&text), sender).await;
        assert_eq!(status, 200, "an editor save from {what}: {body}");
        let dropped = "/api/v1/file-drop?pty=s1&filename=dropped.txt";
        let (status, body) = send(addr, &Method::POST, dropped, Some(&json!("x")), sender).await;
        assert_eq!(status, 200, "a dropped file from {what}: {body}");
    }
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("f.txt")).expect("the saved file"),
        "three\n"
    );
}
