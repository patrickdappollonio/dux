//! `GET /api/v1/build`: which build, and which RUN, of dux this server is.
//!
//! ## Why this exists
//!
//! A browser tab left open across a dux restart keeps running the interface it
//! was served, against a server that is not the one that served it. The events
//! socket reconnects and the app carries on, rendering whatever the new server
//! sends through the old code. When the two builds disagree about the wire shape
//! that is silently wrong, and the tab is the last place anyone would look.
//!
//! So the client reads this endpoint when the tab loads, remembers the answer,
//! and reads it again while it is on the disconnected screen trying to get back.
//! A changed answer means the tab's code no longer matches the server, and the
//! client hard reloads rather than reconnecting in place.
//!
//! ## Why there are two fields and not one
//!
//! `version` alone cannot do it. It is the literal string `development` for every
//! build that is not a tagged release (see `dux-core/build.rs`), and even for
//! releases it moves only when the release number does. Rebuilding dux during
//! development and restarting it, which is exactly the case this exists for,
//! would not move it at all.
//!
//! `process` alone would be enough to catch every restart, and in a development
//! build it is what does all the work: every restart mints a new one, so every
//! restart forces a reload, which is what a developer wants. `version` is kept
//! alongside it because it is the thing a human recognises, and in a release
//! build it is the field that names what actually changed under the user.
//!
//! ## Why not just always reload on reconnect
//!
//! Because a dropped connection is not a restart. A network blip returns to the
//! SAME process, both fields match, and the client reconnects in place rather
//! than throwing away an editor tab somebody had open. That is the only thing
//! this buys over reloading unconditionally, it costs one comparison, and it is
//! worth it.
//!
//! ## Deliberately narrow
//!
//! This says which run of which build is answering, and nothing else. It must not
//! grow into a schema or data-shape version: the interface ships inside the
//! server binary, so a shape change cannot reach a client without a new build and
//! a restart carrying it, which means this check already covers every case a
//! shape version would.
//!
//! ## Status codes
//!
//! - 200, always. Deliberately: this answers from process-local data with no
//!   engine round-trip, so it is available while the engine is still coming up,
//!   which is precisely when a client that has just reconnected asks. An endpoint
//!   that could 503 during startup would fail at the one moment it matters.
//!
//! Served like every other API route. dux has NO authentication of any kind, so
//! nothing here ever 401s: the single-tenant trusted-access model documented in
//! CLAUDE.md. The Host-header allowlist still applies, and the same-origin check
//! covers mutations only, so this GET is not behind it. Neither is authentication.

use axum::{
    Json, Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::server::AppState;

/// What this server reports about itself. Both fields together are the identity
/// the client compares; either one moving means "not the server this tab loaded
/// against".
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity {
    /// The binary's display version, the same string shown under the logo
    /// (`vX.Y.Z` for a release build, `development` otherwise).
    pub version: String,
    /// This RUN of the server. Minted once at first read and never again for the
    /// life of the process, so it is stable across every request a client makes
    /// and different on the other side of a restart.
    pub process: String,
}

/// This process's run id. A `LazyLock` and not a per-request value, and that is
/// the load-bearing part: minting one per REQUEST would make every reconnect
/// look like a restart and hard reload the tab forever.
///
/// `uuid::Uuid::new_v4` is what this crate already reaches for when it needs a
/// server-minted opaque id (the per-connection ids on `/ws/events`).
static PROCESS_ID: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| uuid::Uuid::new_v4().to_string());

/// This server's identity, computed once.
pub fn build_identity() -> &'static BuildIdentity {
    static IDENTITY: std::sync::LazyLock<BuildIdentity> =
        std::sync::LazyLock::new(|| BuildIdentity {
            version: dux_core::display_version().to_string(),
            process: PROCESS_ID.clone(),
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

    /// The wire shape is exactly two keys, and no more. The client compares the
    /// whole document, so a field added here changes what every open tab believes
    /// about the server.
    #[test]
    fn the_body_carries_exactly_version_and_process() {
        let json = serde_json::to_value(build_identity()).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("a JSON object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["process", "version"]);
    }
}
