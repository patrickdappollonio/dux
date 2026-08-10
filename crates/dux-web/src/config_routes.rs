//! REST write verbs for the config-mutating operations the palette / dialogs
//! trigger (Phase 6 of the REST-first migration). These used to ride the retired
//! `/ws` `command` channel (`update_macros`, `persist_global_env`,
//! `set_changes_pane_visible`, `reload_config`); they are now scoped REST verbs,
//! each dispatching the matching [`WireCommand`] via
//! [`EngineHandle::apply_wire_scoped`] with a per-connection [`StatusScope`]
//! derived from the optional `X-Connection-Id` header (the Phase 4 pattern).
//!
//! Every route is served plainly: dux has NO authentication, so none of these
//! ever 401s. The open access is deliberate (the single-tenant trusted-access
//! model in CLAUDE.md), and the app-wide guards are not authentication: a
//! Host-header allowlist stops a malicious web page rebinding DNS into this
//! server, and the same-origin check stops another site driving these verbs from a
//! visitor's browser, but a client sending no `Origin` (curl, a script) bypasses
//! it by design. Any client that can reach the address can rewrite `config.toml`.
//!
//! Routes:
//! - `PUT  /api/v1/macros`           — replace the macro set wholesale.
//! - `PUT  /api/v1/global-env`       — replace the workspace-wide env map.
//! - `PUT  /api/v1/ui/changes-pane`  — set the Changes-pane visibility flag.
//! - `POST /api/v1/config/reload`    — re-read `config.toml` from disk.
//! - `POST /api/v1/defaults/toggle-randomized-pet-name` — flip the random
//!   pet-name default.
//! - `POST /api/v1/ui/toggle-pr-banner-position` — swap the PR banner top/bottom.
//! - `POST /api/v1/ui/agent-sort` — set the web agent-list sort mode (validated).
//! - `POST /api/v1/ui/toggle-github-integration` — flip GitHub PR integration.
//! - `POST /api/v1/ui/toggle-copy-on-select` — flip web-terminal copy-on-select.
//! - `POST /api/v1/ui/toggle-always-show-tab-strip` — flip whether the agent tab
//!   strip always renders, even with a single tab.
//! - `POST /api/v1/config/instance-identity`: set the browser tab title and
//!   favicon color.
//! - `PATCH /api/v1/config/settings`: set explicit values for the grouped
//!   Settings modal's other `[ui]`/`[capabilities]` fields in one request (see
//!   `crates/dux-web/web/src/lib/settingsDescriptors.ts`).
//!
//! On a successful config change the engine emits a `config.changed` event (via
//! the Phase 2 forwarder in `server.rs`), so subscribed clients refetch
//! `/api/v1/bootstrap` — these handlers do not echo the new state in their reply.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use serde::{Deserialize, Serialize};

use dux_core::wire::{SettingsPatch, WireCommand, WireMacroEntry};

use crate::rest_common::scope_from_headers;
use crate::server::AppState;

/// The config-mutation routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/macros", put(update_macros))
        .route("/api/v1/global-env", put(persist_global_env))
        .route("/api/v1/ui/changes-pane", put(set_changes_pane))
        .route("/api/v1/config/reload", post(reload_config))
        .route(
            "/api/v1/defaults/toggle-randomized-pet-name",
            post(toggle_randomized_pet_name_default),
        )
        .route(
            "/api/v1/ui/toggle-pr-banner-position",
            post(toggle_pr_banner_position),
        )
        .route("/api/v1/ui/agent-sort", post(set_agent_sort))
        .route(
            "/api/v1/ui/toggle-github-integration",
            post(toggle_github_integration),
        )
        .route(
            "/api/v1/ui/toggle-copy-on-select",
            post(toggle_copy_on_select),
        )
        .route(
            "/api/v1/ui/toggle-always-show-tab-strip",
            post(toggle_always_show_tab_strip),
        )
        .route(
            "/api/v1/config/instance-identity",
            post(set_instance_identity),
        )
        .route("/api/v1/config/settings", patch(set_settings))
        .route(
            "/api/v1/config/raw",
            // A config.toml is a few KB; 256 KB is generous. The cap stops a
            // client from streaming a multi-MB body that the engine thread would
            // then parse and fsync.
            get(read_raw_config)
                .put(write_raw_config)
                .layer(axum::extract::DefaultBodyLimit::max(256 * 1024)),
        )
}

// ── Macros ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateMacrosBody {
    /// The whole macro set, in order. `WireMacroEntry` is `{name, text, surface}`,
    /// matching the frontend's `MacroView`. The engine validates wholesale
    /// (empty/duplicate names, empty text, unknown surface all rejected).
    entries: Vec<WireMacroEntry>,
}

async fn update_macros(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateMacrosBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::UpdateMacros {
            entries: body.entries,
        },
    )
    .await
}

// ── Global env ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GlobalEnvBody {
    /// The whole workspace-wide env map (replace-wholesale).
    env: BTreeMap<String, String>,
}

async fn persist_global_env(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GlobalEnvBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::PersistGlobalEnv { env: body.env },
    )
    .await
}

// ── Changes pane ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChangesPaneBody {
    visible: bool,
}

async fn set_changes_pane(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangesPaneBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::SetChangesPaneVisible {
            visible: body.visible,
        },
    )
    .await
}

// ── Reload ─────────────────────────────────────────────────────────────────────

/// `POST /api/v1/config/reload`. No body is required (the frontend sends `{}`),
/// so no `Json` extractor — a config reload re-reads `config.toml` from disk.
async fn reload_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::ReloadConfig {}).await
}

// ── Preference toggles ───────────────────────────────────────────────────────
//
// These mirror the TUI palette toggles. Each is a parameterless POST: the server
// owns the current value and flips it (so two surfaces never disagree about the
// "next" state), persists, and emits `config.changed` so every client refetches
// the bootstrap document. The frontend confirms via the routed status toast.

/// `POST /api/v1/defaults/toggle-randomized-pet-name`. Flip the random pet-name
/// default (`defaults.enable_randomized_pet_name_by_default`).
async fn toggle_randomized_pet_name_default(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::ToggleRandomizedPetNameDefault {},
    )
    .await
}

/// `POST /api/v1/ui/toggle-pr-banner-position`. Swap the PR banner between the
/// top and bottom of the agent pane (`ui.pr_banner_position`).
async fn toggle_pr_banner_position(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::TogglePrBannerPosition {}).await
}

#[derive(Deserialize)]
struct AgentSortBody {
    sort: String,
}

/// `POST /api/v1/ui/agent-sort`. Set the web agent-list sort mode
/// (`ui.agent_sort`) to an explicit value. The engine validates it and rejects
/// unknown modes. The sidebar's sort control and a drag-reorder both call this.
async fn set_agent_sort(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AgentSortBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::SetAgentSort { sort: body.sort },
    )
    .await
}

/// `POST /api/v1/ui/toggle-github-integration`. Flip GitHub PR integration
/// (`ui.github_integration`) and its engine-side PR-sync side effects.
async fn toggle_github_integration(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::ToggleGithubIntegration {}).await
}

/// `POST /api/v1/ui/toggle-copy-on-select`. Flip whether selecting text in the
/// web terminal auto-copies it (`ui.copy_on_select`).
async fn toggle_copy_on_select(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::ToggleCopyOnSelect {}).await
}

/// `POST /api/v1/ui/toggle-always-show-tab-strip`. Flip whether the agent tab
/// strip is always shown, even with a single tab (`ui.always_show_tab_strip`).
async fn toggle_always_show_tab_strip(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    dispatch(&state, &headers, WireCommand::ToggleAlwaysShowTabStrip {}).await
}

// ── Instance identity (customize-webapp dialog) ──────────────────────────────

/// The instance identity body. Both fields are `#[serde(default)]` so a single-field
/// body (`{"favicon":"amber"}`) or an empty body (`{}`) both deserialize — the
/// handler only touches the fields that are present, and an empty body is a no-op.
#[derive(Deserialize, Default)]
struct InstanceIdentityBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    favicon: Option<String>,
}

/// `POST /api/v1/config/instance-identity`. Persist this dux instance's browser
/// tab title (`config.server.title`) and favicon color (`config.server.favicon`).
/// Bare `200` on success; plain-text `400` on rejection (an unknown favicon color)
/// via the shared `dispatch`. The engine validates + normalizes and fires
/// `config.changed` so every tab refetches its title + favicon.
async fn set_instance_identity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InstanceIdentityBody>,
) -> Response {
    dispatch(
        &state,
        &headers,
        WireCommand::SetInstanceIdentity {
            title: body.title,
            favicon: body.favicon,
        },
    )
    .await
}

// ── Settings PATCH (grouped Settings modal) ──────────────────────────────────
//
// Body shape decision (see the plan's section 6/open-risk-2 tension between a
// flat dotted-key map and a typed struct that rejects unknown keys): a
// `HashMap<String, Value>` can't enforce per-field types or reject unknown
// keys, so this uses NESTED typed sub-structs: `{"ui": {...}, "capabilities":
// {...}}`, each `#[serde(default, deny_unknown_fields)]` with every field
// `Option<T>`. `#[serde(rename = "...")]` dotted keys on a flat struct fight
// `deny_unknown_fields` in practice here (the two groups' fields would need to
// live in one flat struct to dotted-rename cleanly, which reintroduces the
// "which endpoint owns title/favicon" ambiguity), so nesting by config section
// is the form that compiles cleanly AND matches how the fields are actually
// grouped in `config.toml` (`[ui]` / `[capabilities]`).

/// The `[ui]` half of a settings-PATCH body. Every field is optional; an
/// absent field is left untouched. Unknown fields are rejected (400) so a
/// typo or a client/server drift surfaces immediately instead of silently
/// no-opping.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct UiSettingsPatch {
    copy_on_select: Option<bool>,
    compose_bar: Option<bool>,
    mobile_top_bar: Option<bool>,
    mobile_accessory_bar: Option<bool>,
    /// Whether the agent upload directory keeps a self-ignoring `.gitignore`.
    /// Its companion `upload_directory` is deliberately not settable here: it
    /// is a path, and the web has no directory picker to edit one with.
    upload_write_gitignore: Option<bool>,
    /// How many characters a text paste onto an agent pane may run to before
    /// the web saves it as a file and pastes the path. Out-of-range values are
    /// clamped engine-side (see `normalized_upload_pasted_text_chars`), not
    /// rejected here.
    upload_pasted_text_chars: Option<usize>,
    auto_reopen_agents: Option<bool>,
    show_changes_pane: Option<bool>,
    always_show_tab_strip: Option<bool>,
    status_clear_seconds: Option<u16>,
    attention_grace_seconds: Option<u64>,
    attention_indicator: Option<bool>,
    attention_on_bell: Option<bool>,
    pr_banner_position: Option<String>,
    /// Suppresses the AUTOMATIC first-run welcome screen only; the app menu's
    /// on-demand entry still opens it.
    disable_automated_welcome_screen: Option<bool>,
    /// Suppresses the AUTOMATIC what's-new screen only; the app menu's on-demand
    /// entry still opens it.
    disable_release_notes: Option<bool>,
    /// A font name installed on the viewing device, placed ahead of dux's
    /// bundled web terminal font stack. Empty string is a valid value (it
    /// means "use the bundled stack only"). Web UI only.
    terminal_font_family: Option<String>,
    /// The web terminal's font size in pixels. Out-of-range values are
    /// normalized engine-side (see `normalized_terminal_font_size`), not
    /// rejected here.
    terminal_font_size: Option<u16>,
}

/// The `[capabilities]` half of a settings-PATCH body. Same optional/
/// unknown-field-rejecting shape as [`UiSettingsPatch`].
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct CapabilitiesSettingsPatch {
    web_notifications: Option<bool>,
    hyperlinks: Option<bool>,
}

/// The `[defaults]` half of a settings-PATCH body. Same optional/
/// unknown-field-rejecting shape as [`UiSettingsPatch`]. `provider` is the
/// GLOBAL default provider for new agents in projects without a
/// project-specific override (mirrors the TUI's `change-default-provider`
/// palette command); it is validated engine-side against the configured
/// provider list, the same source `BootstrapView::available_providers` is
/// built from. This is distinct from a project's own `default_provider`
/// override, which has its own dedicated wire path.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct DefaultsSettingsPatch {
    enable_randomized_pet_name_by_default: Option<bool>,
    provider: Option<String>,
}

/// `PATCH /api/v1/config/settings` body:
/// `{"ui": {...}, "capabilities": {...}, "defaults": {...}}`, every group
/// optional, every leaf field optional. `title`/`favicon` are deliberately
/// absent here, they stay on `POST /api/v1/config/instance-identity`, and
/// `ui.github_integration` is deliberately absent too: flipping it arms or
/// disarms background PR syncing, so it keeps its dedicated
/// `POST /api/v1/config/toggle-github-integration` endpoint rather than forking
/// that side-effect logic into this route.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SettingsBody {
    ui: UiSettingsPatch,
    capabilities: CapabilitiesSettingsPatch,
    defaults: DefaultsSettingsPatch,
    /// Top-level because it is not a settings field: it asks the engine to
    /// emit no info status for this request, and the engine honors it only
    /// for a patch confined to the two mobile-bar fields (see
    /// `SettingsPatch::quiet`), so it cannot silence any other settings
    /// write. Sent by the web's mobile bar toggles, whose feedback is the
    /// bar itself moving.
    quiet: bool,
}

/// `PATCH /api/v1/config/settings`. Set explicit values for the Settings
/// modal's `[ui]`/`[capabilities]` fields in one request (see
/// `crates/dux-web/web/src/lib/settingsDescriptors.ts` for the exact field
/// set the modal renders). Any field omitted from the body is left untouched;
/// a body with no fields present is a no-op `200`. `200` on success; plain-text
/// `400` on a validation error (unknown enum value, unknown/mistyped field) via
/// the shared `dispatch`. A rejected patch mutates nothing. The engine clamps
/// numeric fields to a documented ceiling server-side, so the client should
/// treat its own bounds as UX-only and re-seed from the post-save bootstrap
/// refetch for the authoritative saved value.
async fn set_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<SettingsBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // axum's default `Json` extractor answers a deserialize failure (an
    // unknown field caught by `deny_unknown_fields`, or a field of the wrong
    // type) with 422 Unprocessable Entity. This route deliberately maps that
    // rejection to plain-text 400 instead. This is an intentional DIVERGENCE
    // from the other `Json<...>`-extracting routes in this file (e.g.
    // `set_instance_identity`), which still fall through to axum's default
    // 422 on a malformed body, not an attempt to make every config-mutation
    // route return the same status for a bad body. The divergence is
    // acceptable here because this route is the one that layers typed,
    // `deny_unknown_fields` nested sub-structs on `Json` (see the body-shape
    // decision above), so a client typo or client/server field-set drift is
    // far more likely to surface as a deserialize rejection on this route
    // than on the others' flat bodies; mapping it to 400 with the
    // rejection's message as the body matches this route's own hand-rolled
    // 400 validation failures, so a caller of `set_settings` only ever needs
    // to branch on "ok" vs "4xx with a message" for THIS route.
    let Json(body) = match body {
        Ok(json) => json,
        Err(rejection) => {
            return (StatusCode::BAD_REQUEST, rejection.body_text()).into_response();
        }
    };
    dispatch(
        &state,
        &headers,
        // The regrouping from the body's config-section groups onto the flat
        // patch is real work, so it stays hand-written: this is the contract
        // boundary between the public HTTP shape and the wire command.
        WireCommand::SetSettings(SettingsPatch {
            copy_on_select: body.ui.copy_on_select,
            compose_bar: body.ui.compose_bar,
            mobile_top_bar: body.ui.mobile_top_bar,
            mobile_accessory_bar: body.ui.mobile_accessory_bar,
            upload_write_gitignore: body.ui.upload_write_gitignore,
            upload_pasted_text_chars: body.ui.upload_pasted_text_chars,
            auto_reopen_agents: body.ui.auto_reopen_agents,
            show_changes_pane: body.ui.show_changes_pane,
            web_notifications: body.capabilities.web_notifications,
            always_show_tab_strip: body.ui.always_show_tab_strip,
            status_clear_seconds: body.ui.status_clear_seconds,
            attention_grace_seconds: body.ui.attention_grace_seconds,
            attention_indicator: body.ui.attention_indicator,
            attention_on_bell: body.ui.attention_on_bell,
            pr_banner_position: body.ui.pr_banner_position,
            hyperlinks: body.capabilities.hyperlinks,
            enable_randomized_pet_name_by_default: body
                .defaults
                .enable_randomized_pet_name_by_default,
            default_provider: body.defaults.provider,
            disable_automated_welcome_screen: body.ui.disable_automated_welcome_screen,
            disable_release_notes: body.ui.disable_release_notes,
            terminal_font_family: body.ui.terminal_font_family,
            terminal_font_size: body.ui.terminal_font_size,
            quiet: body.quiet,
        }),
    )
    .await
}

// ── Raw config editor (Monaco) ───────────────────────────────────────────────

#[derive(Serialize)]
struct RawConfigBody {
    /// The raw `config.toml` text, verbatim from disk (or the plain render of the
    /// running config when no file exists yet).
    content: String,
}

#[derive(Deserialize)]
struct WriteRawConfigBody {
    content: String,
}

/// `GET /api/v1/config/raw`. Return the raw `config.toml` text for the Monaco
/// editor. Served like every other config route (no authentication) but takes no
/// body. A
/// read failure (permission/IO, or the engine being gone) is a `503` so the
/// editor surfaces an error instead of opening on blank content.
async fn read_raw_config(State(state): State<AppState>) -> Response {
    match state.engine.read_raw_config().await {
        Ok(content) => Json(RawConfigBody { content }).into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

/// `PUT /api/v1/config/raw`. Validate (`toml::from_str::<Config>`) and write the
/// raw `config.toml` text verbatim. `200 OK` on success; `400` with the parse/IO
/// error otherwise. This PERSISTS only — the engine does NOT adopt the change and
/// emits no `config.changed`; the running config is untouched until the user
/// explicitly runs `POST /api/v1/config/reload`. Reload is the single apply point.
async fn write_raw_config(
    State(state): State<AppState>,
    Json(body): Json<WriteRawConfigBody>,
) -> Response {
    match state.engine.write_raw_config(body.content).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

// ── Shared dispatch ─────────────────────────────────────────────────────────────

/// Dispatch a config-mutating wire command, scoping its status toasts to the
/// originating connection. `200 OK` on success; `400` with the engine's
/// user-facing validation message otherwise.
async fn dispatch(state: &AppState, headers: &HeaderMap, cmd: WireCommand) -> Response {
    match state
        .engine
        .apply_wire_scoped(cmd, scope_from_headers(headers, &state.connections))
        .await
    {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::test_support::router_no_auth;

    fn json_req(method: &str, uri: &str, body: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn update_macros_accepts_a_valid_set() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/macros",
                r#"{"entries":[{"name":"greet","text":"hi","surface":"agent"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_macros_rejects_an_empty_name_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/macros",
                r#"{"entries":[{"name":"","text":"hi","surface":"agent"}]}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn persist_global_env_accepts_a_map() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/global-env",
                r#"{"env":{"FOO":"bar"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn set_changes_pane_accepts_a_flag() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/ui/changes-pane",
                r#"{"visible":false}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Read the raw `config.toml` text back through `GET /api/v1/config/raw` so a
    /// persistence assertion sees what actually landed on disk / in the running
    /// config, not just the POST's status code.
    async fn read_raw_config_text(app: &Router) -> String {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config/raw")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        v["content"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn instance_identity_accepts_a_single_field_body() {
        // `#[serde(default)]` on both fields: a favicon-only body deserializes.
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/config/instance-identity",
                r#"{"favicon":"amber"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn instance_identity_persists_a_valid_post() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/config/instance-identity",
                r#"{"title":"dux prod","favicon":"amber"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains("title = \"dux prod\""),
            "title should persist: {raw}"
        );
        assert!(
            raw.contains("favicon = \"amber\""),
            "favicon should persist: {raw}"
        );
    }

    #[tokio::test]
    async fn instance_identity_empty_body_resets_to_default() {
        // The dialog's "Reset to default" button POSTs empty strings for both
        // fields. Empty title normalizes back to "dux" and empty favicon back to
        // "" (the default full-colour duck). First set a non-default identity, then
        // reset, and confirm the re-read config reflects the defaults.
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/config/instance-identity",
                r#"{"title":"x","favicon":"amber"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/config/instance-identity",
                r#"{"title":"","favicon":""}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains("title = \"dux\""),
            "empty title should reset to \"dux\": {raw}"
        );
        assert!(
            raw.contains("favicon = \"\""),
            "empty favicon should reset to the default (empty): {raw}"
        );
    }

    #[tokio::test]
    async fn instance_identity_rejects_bad_favicon_and_leaves_config_unchanged() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/config/instance-identity",
                r#"{"favicon":"mauve"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let after = read_raw_config_text(&app).await;
        assert_eq!(before, after, "a rejected favicon must not mutate config");
        assert!(!after.contains("mauve"));
    }

    #[tokio::test]
    async fn instance_identity_empty_body_is_a_noop() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req("POST", "/api/v1/config/instance-identity", "{}"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let after = read_raw_config_text(&app).await;
        assert_eq!(before, after, "an empty body must not mutate config");
    }

    #[tokio::test]
    async fn reload_config_accepts_an_empty_body() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req("POST", "/api/v1/config/reload", "{}"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_raw_config_returns_ok() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config/raw")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_then_write_round_trips_with_200() {
        let (_tmp, app) = router_no_auth();
        // Read the current raw config and confirm the body carries `content`.
        let get = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/config/raw")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(get.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let content = parsed["content"]
            .as_str()
            .expect("read body must carry a content string")
            .to_string();
        assert!(!content.is_empty(), "content must not be empty");

        // Write it back unchanged: valid TOML with an unchanged [server] section,
        // so the happy path returns 200 (exercises the Ok arm of the persist).
        let body = serde_json::json!({ "content": content }).to_string();
        let put = app
            .oneshot(json_req("PUT", "/api/v1/config/raw", &body))
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn write_raw_config_rejects_invalid_toml_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PUT",
                "/api/v1/config/raw",
                r#"{"content":"this is = = not valid toml"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Settings PATCH (grouped Settings modal) ──────────────────────────────

    #[tokio::test]
    async fn set_settings_accepts_a_valid_ui_patch() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"copy_on_select":false,"always_show_tab_strip":true,"pr_banner_position":"top"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("copy_on_select = false"), "raw: {raw}");
        assert!(raw.contains("always_show_tab_strip = true"), "raw: {raw}");
        assert!(raw.contains("pr_banner_position = \"top\""), "raw: {raw}");
    }

    /// The top-level `quiet` flag rides beside the groups (it is not a
    /// settings field) and still persists the mobile-bar write; the engine
    /// drops the info status for such a request (pinned in
    /// `dux_core::wire`'s `set_settings_quiet_*` tests).
    #[tokio::test]
    async fn set_settings_accepts_the_quiet_flag() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"mobile_top_bar":false},"quiet":true}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("mobile_top_bar = false"), "raw: {raw}");
    }

    #[tokio::test]
    async fn set_settings_clamps_out_of_range_status_clear_seconds() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"status_clear_seconds":65535}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains(&format!(
                "status_clear_seconds = {}",
                dux_core::config::MAX_STATUS_CLEAR_SECONDS
            )),
            "expected the clamped ceiling to persist: {raw}"
        );
    }

    #[tokio::test]
    async fn set_settings_degrades_an_out_of_range_terminal_font_size_to_the_default() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"terminal_font_size":200}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains(&format!(
                "terminal_font_size = {}",
                dux_core::config::DEFAULT_TERMINAL_FONT_SIZE
            )),
            "expected the out-of-range value to degrade to the default: {raw}"
        );
    }

    #[tokio::test]
    async fn set_settings_accepts_zero_for_attention_grace_seconds() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"attention_grace_seconds":0}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains("attention_grace_seconds = 0"),
            "0 must persist as a real value, not a clamp/default: {raw}"
        );
    }

    /// CROSS-LANGUAGE PIN. The Preferences modal's PATCH keys live twice: in
    /// `SettingsBody` here, and in the `writeTarget: "settings"` descriptors in
    /// `crates/dux-web/web/src/lib/settingsDescriptors.ts`. There is no codegen
    /// between them, so both halves are pinned by a loud test. This is the
    /// server half; the twin is the set-equality assertion in
    /// `settingsDescriptors.test.ts` ("the settings-PATCH key set matches the
    /// server's accepted fields").
    ///
    /// This PATCHes every key that modal can emit, in one body, across all
    /// three groups. Because `SettingsBody` is `deny_unknown_fields`, a key the
    /// modal sends but the server dropped fails here with a 400 rather than
    /// being silently ignored. Each value is asserted to land, so a key that
    /// parses but is never mapped fails too.
    ///
    /// `ui.show_changes_pane` is deliberately absent: the server accepts it,
    /// but the modal routes that row to the dedicated Changes-pane endpoint.
    /// `ui.github_integration` and `server.title`/`favicon` are absent for the
    /// same reason, each keeping its own endpoint.
    #[tokio::test]
    async fn set_settings_accepts_every_key_the_modal_can_send() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{
                    "ui": {
                        "copy_on_select": false,
                        "compose_bar": false,
                        "mobile_top_bar": false,
                        "mobile_accessory_bar": false,
                        "upload_write_gitignore": false,
                        "auto_reopen_agents": true,
                        "always_show_tab_strip": true,
                        "status_clear_seconds": 42,
                        "attention_grace_seconds": 11,
                        "attention_indicator": false,
                        "attention_on_bell": false,
                        "pr_banner_position": "top",
                        "disable_automated_welcome_screen": true,
                        "disable_release_notes": true,
                        "terminal_font_family": "Fira Code",
                        "terminal_font_size": 18
                    },
                    "capabilities": {
                        "web_notifications": true,
                        "hyperlinks": false
                    },
                    "defaults": {
                        "enable_randomized_pet_name_by_default": true,
                        "provider": "codex"
                    }
                }"#,
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "every key the modal can send must be accepted"
        );

        let raw = read_raw_config_text(&app).await;
        for expected in [
            "copy_on_select = false",
            "compose_bar = false",
            "mobile_top_bar = false",
            "mobile_accessory_bar = false",
            "upload_write_gitignore = false",
            "auto_reopen_agents = true",
            "always_show_tab_strip = true",
            "status_clear_seconds = 42",
            "attention_grace_seconds = 11",
            "attention_indicator = false",
            "attention_on_bell = false",
            "pr_banner_position = \"top\"",
            "web_notifications = true",
            "hyperlinks = false",
            "enable_randomized_pet_name_by_default = true",
            "provider = \"codex\"",
            "disable_automated_welcome_screen = true",
            "disable_release_notes = true",
            "terminal_font_family = \"Fira Code\"",
            "terminal_font_size = 18",
        ] {
            assert!(
                raw.contains(expected),
                "expected {expected:?} to persist, got:\n{raw}"
            );
        }
    }

    /// The `defaults` group is the one that drifted out of the TS body type
    /// unnoticed, so pin it end to end at the HTTP boundary on its own.
    #[tokio::test]
    async fn set_settings_applies_the_defaults_group_end_to_end() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"defaults":{"enable_randomized_pet_name_by_default":true,"provider":"codex"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains("enable_randomized_pet_name_by_default = true"),
            "the defaults group must persist: {raw}"
        );
        assert!(
            raw.contains("provider = \"codex\""),
            "the defaults group must persist: {raw}"
        );
    }

    /// A rejected value must take the WHOLE patch down, including valid fields
    /// in OTHER groups. `set_settings_rejects_an_unconfigured_default_provider_with_400`
    /// covers the lone-invalid-field case; this covers the all-or-nothing part,
    /// which is the half a partial apply would break.
    #[tokio::test]
    async fn set_settings_rejects_a_whole_patch_when_one_group_is_invalid() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"copy_on_select":false},"defaults":{"provider":"not-a-real-provider"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let after = read_raw_config_text(&app).await;
        assert_eq!(
            before, after,
            "a rejected provider must not partially apply the rest of the patch"
        );
    }

    #[tokio::test]
    async fn set_settings_rejects_unknown_enum_value_with_400() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"pr_banner_position":"sideways"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let after = read_raw_config_text(&app).await;
        assert_eq!(
            before, after,
            "a rejected enum value must not mutate config"
        );
    }

    #[tokio::test]
    async fn set_settings_empty_patch_is_a_noop_200() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req("PATCH", "/api/v1/config/settings", "{}"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let after = read_raw_config_text(&app).await;
        assert_eq!(before, after, "an empty patch must not mutate config");
    }

    #[tokio::test]
    async fn set_settings_ignores_absent_fields() {
        let (_tmp, app) = router_no_auth();

        // Set the PR banner position first.
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"pr_banner_position":"top"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // A second patch that only touches an unrelated field must leave the
        // PR banner position untouched.
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"status_clear_seconds":8}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("pr_banner_position = \"top\""), "raw: {raw}");
        assert!(raw.contains("status_clear_seconds = 8"), "raw: {raw}");
    }

    #[tokio::test]
    async fn set_settings_rejects_unknown_top_level_key_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"server":{"title":"hacked"}}"#,
            ))
            .await
            .unwrap();
        // `deny_unknown_fields` rejects a "server" group outright: title/favicon
        // stay on the dedicated instance-identity endpoint, not this one.
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_settings_rejects_unknown_field_within_a_group_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"ui":{"not_a_real_field":true}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn set_settings_accepts_a_capabilities_patch() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"capabilities":{"web_notifications":false,"hyperlinks":false}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("web_notifications = false"), "raw: {raw}");
        assert!(raw.contains("hyperlinks = false"), "raw: {raw}");
    }

    // The `[defaults]` group is the first non-`ui`/`capabilities` group on this
    // PATCH. It exists because the Preferences dialog now carries the random
    // pet-name default, which used to be a web command-palette toggle. Unlike
    // `github_integration` (whose flip has PR-sync side effects and therefore
    // keeps its dedicated endpoint), this is a plain field write, so it rides
    // the generic settings PATCH.
    #[tokio::test]
    async fn set_settings_accepts_a_defaults_patch() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"defaults":{"enable_randomized_pet_name_by_default":true}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(
            raw.contains("enable_randomized_pet_name_by_default = true"),
            "raw: {raw}"
        );
    }

    #[tokio::test]
    async fn set_settings_rejects_unknown_field_within_defaults_with_400() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"defaults":{"not_a_real_field":true}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // `defaults.provider` is the GLOBAL default provider (distinct from a
    // project's own `default_provider` override). It rides this generic patch
    // because, like the pet-name default, flipping it is a plain field write
    // with no side effects.
    #[tokio::test]
    async fn set_settings_accepts_a_valid_default_provider_patch() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"defaults":{"provider":"codex"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("provider = \"codex\""), "raw: {raw}");
    }

    // Test engines default to the four built-in providers (claude, codex,
    // opencode, copilot; see `Config::default()`/`default_provider_commands()`),
    // so a name outside that set is unconfigured and must be rejected with a
    // plain-text 400, mirroring `set_settings_rejects_unknown_enum_value_with_400`
    // for `pr_banner_position`.
    #[tokio::test]
    async fn set_settings_rejects_an_unconfigured_default_provider_with_400() {
        let (_tmp, app) = router_no_auth();
        let before = read_raw_config_text(&app).await;

        let resp = app
            .clone()
            .oneshot(json_req(
                "PATCH",
                "/api/v1/config/settings",
                r#"{"defaults":{"provider":"not-a-real-provider"}}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let after = read_raw_config_text(&app).await;
        assert_eq!(
            before, after,
            "an unconfigured provider must not mutate config"
        );
    }

    #[tokio::test]
    async fn preference_toggles_accept_a_post_with_no_body() {
        for uri in [
            "/api/v1/defaults/toggle-randomized-pet-name",
            "/api/v1/ui/toggle-pr-banner-position",
            "/api/v1/ui/toggle-github-integration",
            "/api/v1/ui/toggle-copy-on-select",
            "/api/v1/ui/toggle-always-show-tab-strip",
        ] {
            let (_tmp, app) = router_no_auth();
            let resp = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "POST {uri}");
        }
    }
}
