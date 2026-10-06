//! `GET /api/v1/build`: which build, and which run, of dux this server is. A
//! client reads it on load and again while reconnecting; a changed answer means
//! its code no longer matches the server, so it hard reloads instead of
//! reconnecting a tab into a build that may disagree about the wire shape.
//!
//! It identifies the server run and is not a schema version: the body is exactly
//! `version`, `process` and `api`, the REST API version the separately shipped
//! command line checks.
//!
//! Always 200: it answers from process-local data with no engine round-trip, so
//! it is available while the engine is still coming up.

use axum::{
    Json, Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::server::AppState;

/// What this server reports about itself. The browser reads a change in
/// `version` or `process` as a different server run.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// The binary's display version, the same string shown under the logo
    /// (`vX.Y.Z` for a release build, `development` otherwise).
    pub version: String,
    /// This RUN of the server. Minted once at first read and never again for the
    /// life of the process, so it is stable across every request a client makes
    /// and different on the other side of a restart.
    pub process: String,
    /// The REST API version, which the command line checks before anything
    /// else; the browser ignores it.
    pub api: u64,
}

/// This process's run id, minted once per process: a per-request value would make
/// every reconnect look like a restart and hard reload the tab forever.
static PROCESS_ID: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// This server's identity, computed once.
pub fn build_identity() -> &'static BuildIdentity {
    static IDENTITY: std::sync::LazyLock<BuildIdentity> =
        std::sync::LazyLock::new(|| BuildIdentity {
            version: dux_core::display_version().to_string(),
            process: PROCESS_ID.clone(),
            api: dux_core::client::API_VERSION,
        });
    &IDENTITY
}

/// The build-identity read route.
pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/build", get(get_build))
}

async fn get_build() -> Response {
    // `no-store`, not `no-cache`: this is the probe a reconnecting client uses to
    // decide whether its own code is stale, so a cached answer would report what
    // the tab already believes and defeat the check entirely.
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(build_identity()),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property an obvious implementation gets wrong: the run id is minted
    /// ONCE per process, not per read.
    ///
    /// A per-request uuid would satisfy "a restarted server reports something
    /// different" perfectly well, and would also report something different to
    /// the very same tab on every single reconnect, hard reloading it forever and
    /// destroying whatever its user was in the middle of. This is the test that
    /// separates the two.
    #[test]
    fn the_run_id_is_stable_for_the_life_of_the_process() {
        let first = build_identity().clone();
        let second = build_identity().clone();
        let third = build_identity().clone();
        assert_eq!(first, second);
        assert_eq!(second, third);
        assert!(!first.process.is_empty(), "the run id must not be empty");
        assert!(!first.version.is_empty(), "the version must not be empty");
    }

    /// The version is the binary's, not a fresh string: it is the field a human
    /// recognises and the one that names a release upgrade.
    #[test]
    fn the_version_is_the_binarys_display_version() {
        assert_eq!(build_identity().version, dux_core::display_version());
    }

    /// The wire shape is exactly these three keys, and no more. The browser
    /// compares the version and the run, and the command line refuses a dux
    /// whose `api` differs from its own.
    #[test]
    fn the_body_carries_exactly_version_process_and_api_one() {
        let json = serde_json::to_value(build_identity()).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("a JSON object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["api", "process", "version"]);
        assert_eq!(json["api"], 1);
    }
}
