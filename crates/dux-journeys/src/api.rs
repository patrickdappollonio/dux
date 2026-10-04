//! The REST calls several journeys share, made the way the web UI makes them.

use std::time::Duration;

use serde_json::{Value, json};

use crate::client::Client;
use crate::util::eventually;

/// A seeded repository every journey container has (the preview entrypoint
/// creates it on first boot).
pub const DEMO_REPO: &str = "/repos/demo-api";

/// `GET /api/v1/projects`, panicking unless it answered with a list.
pub async fn projects(client: &Client) -> Vec<Value> {
    let response = client.get("/api/v1/projects").await;
    assert_eq!(
        response.status,
        200,
        "listing projects: {}",
        response.describe()
    );
    response
        .json()
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("projects should be a list: {}", response.body))
}

/// Add the demo repository as a project running the fake provider, and return
/// the project's id.
pub async fn add_demo_project(client: &Client) -> String {
    let response = client
        .post_json(
            "/api/v1/projects",
            &json!({ "path": DEMO_REPO, "name": "demo-api" }),
        )
        .await;
    assert!(
        (200..300).contains(&response.status),
        "adding the demo project: {}",
        response.describe()
    );
    let id = response.json()["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the new project has an id: {}", response.body))
        .to_string();
    let response = client
        .patch_json(
            &format!("/api/v1/projects/{id}"),
            &json!({ "provider": "fake" }),
        )
        .await;
    assert!(
        (200..300).contains(&response.status),
        "pointing the project at the fake provider: {}",
        response.describe()
    );
    id
}

/// Create an agent called `name` in `project_id` and wait until it has a tab.
/// Returns the session as the server lists it.
pub async fn create_agent(client: &Client, project_id: &str, name: &str) -> Value {
    let response = client
        .post_json(
            "/api/v1/sessions",
            &json!({
                "kind": "new",
                "project_id": project_id,
                "name": name,
                "copy_uncommitted_changes": false,
            }),
        )
        .await;
    assert!(
        (200..300).contains(&response.status),
        "creating agent {name}: {}",
        response.describe()
    );
    eventually(
        &format!("the agent {name} to appear with a tab"),
        Duration::from_secs(90),
        || async {
            let response = client.get("/api/v1/sessions").await;
            if response.status != 200 {
                return None;
            }
            response
                .json()
                .as_array()?
                .iter()
                .find(|s| s["title"].as_str() == Some(name))
                .filter(|s| s["tabs"].as_array().is_some_and(|t| !t.is_empty()))
                .cloned()
        },
    )
    .await
}

/// The id of a session as the server lists it.
pub fn session_id(session: &Value) -> String {
    session["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a session has an id: {session}"))
        .to_string()
}
