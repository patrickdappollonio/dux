//! REST write verbs for the config-mutating operations. Each dispatches its
//! [`WireCommand`] through [`EngineHandle::apply_wire_scoped`] with a
//! per-connection [`StatusScope`] taken from the optional `X-Connection-Id`
//! header, the same pattern as `session_actions`.
//!
//! Any client that can reach the address can rewrite `config.toml`: that follows
//! from the single-tenant trusted-access model, and the Host allowlist and
//! same-origin check are not authentication.
//!
//! A successful config change makes the engine emit `config.changed`, so clients
//! refetch `/api/v1/bootstrap`; no handler here echoes the new state, except the
//! version a macro or global environment save leaves its set at.
//!
//! The macro list and the global environment each have two ways in. The whole
//! set (`PUT /api/v1/macros`, `PUT /api/v1/global-env`) is what the browser's
//! dialogs save: they send the `macros_version` or `global_env_version` they
//! read, and a set that changed since is refused with `409 {"error":"changed",
//! "message"}`, nothing written. One entry (`PUT` and `DELETE` on
//! `/api/v1/macros/{name}` and `/api/v1/global-env/{name}`) is what a script
//! changes, leaving the rest of the set alone; an entry that is not there is a
//! `404`, and `?operation=1` answers an operation record.
//!
//! Three reads list what the command line's `dux providers ls`, `dux keys ls`
//! and `dux themes ls` print through a remote, built from this dux's own
//! `config.toml` by [`dux_core::config_resources`], the code that lists them
//! on the command line's own machine.

use std::collections::BTreeMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use serde::{Deserialize, Serialize};

use dux_core::operations::OperationKind;
use dux_core::wire::{SettingsPatch, WireCommand, WireMacroEntry};

use crate::rest_common::{OperationQuery, operation_accepted, refusal, scope_from_headers};
use crate::server::AppState;

/// The config-mutation routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/macros", put(update_macros))
        .route("/api/v1/macros/{name}", put(set_macro).delete(remove_macro))
        .route("/api/v1/global-env", put(persist_global_env))
        .route(
            "/api/v1/global-env/{name}",
            put(set_global_env_var).delete(remove_global_env_var),
        )
        .route("/api/v1/ui/changes-pane", put(set_changes_pane))
        .route("/api/v1/config/reload", post(reload_config))
        .route("/api/v1/config/providers", get(list_providers))
        .route("/api/v1/config/keys", get(list_keys))
        .route("/api/v1/config/themes", get(list_themes))
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
        .route("/api/v1/github/recheck", post(recheck_github))
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
        .route("/api/v1/server/tailscale-mode", post(set_tailscale_mode))
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
    /// The bootstrap's `macros_version` the edit started from. Present, a list
    /// that changed since is refused with `409 {"error":"changed"}` and nothing
    /// is written; absent, the save replaces whatever is there.
    #[serde(default)]
    version: Option<String>,
}

/// `PUT /api/v1/macros`: replace the whole list. `200 {"version"}` with the
/// version the list is at now, for the editor's next save.
async fn update_macros(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<UpdateMacrosBody>,
) -> Response {
    save_whole_set(
        &state,
        &headers,
        WireCommand::UpdateMacros {
            entries: body.entries,
            version: body.version,
        },
    )
    .await
}

#[derive(Deserialize)]
struct SetMacroBody {
    text: String,
    /// `agent` | `terminal` | `both`.
    surface: String,
}

/// `PUT /api/v1/macros/{name}`: add the macro, or replace it in place. The
/// command line's `macros add`; the rest of the list is untouched.
async fn set_macro(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(operation): Query<OperationQuery>,
    headers: HeaderMap,
    Json(body): Json<SetMacroBody>,
) -> Response {
    change_one_entry(
        &state,
        &headers,
        &operation,
        WireCommand::SetMacro {
            name,
            text: body.text,
            surface: body.surface,
        },
        OperationKind::MacroSet,
    )
    .await
}

/// `DELETE /api/v1/macros/{name}`: remove one macro; `404` when there is none.
async fn remove_macro(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(operation): Query<OperationQuery>,
    headers: HeaderMap,
) -> Response {
    change_one_entry(
        &state,
        &headers,
        &operation,
        WireCommand::RemoveMacro { name },
        OperationKind::MacroRemove,
    )
    .await
}

// ── Global env ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GlobalEnvBody {
    /// The whole workspace-wide env map (replace-wholesale).
    env: BTreeMap<String, String>,
    /// The bootstrap's `global_env_version` the edit started from, with the
    /// same meaning as the macro list's.
    #[serde(default)]
    version: Option<String>,
}

/// `PUT /api/v1/global-env`: replace the whole table. Answers as
/// `PUT /api/v1/macros` does.
async fn persist_global_env(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<GlobalEnvBody>,
) -> Response {
    save_whole_set(
        &state,
        &headers,
        WireCommand::PersistGlobalEnv {
            env: body.env,
            version: body.version,
        },
    )
    .await
}

#[derive(Deserialize)]
struct SetGlobalEnvVarBody {
    value: String,
}

/// `PUT /api/v1/global-env/{name}`: set one variable. The value is never
/// echoed in the answer or in any status.
async fn set_global_env_var(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(operation): Query<OperationQuery>,
    headers: HeaderMap,
    Json(body): Json<SetGlobalEnvVarBody>,
) -> Response {
    change_one_entry(
        &state,
        &headers,
        &operation,
        WireCommand::SetGlobalEnvVar {
            name,
            value: body.value,
        },
        OperationKind::EnvSet,
    )
    .await
}

/// `DELETE /api/v1/global-env/{name}`: remove one variable; `404` when there
/// is none.
async fn remove_global_env_var(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(operation): Query<OperationQuery>,
    headers: HeaderMap,
) -> Response {
    change_one_entry(
        &state,
        &headers,
        &operation,
        WireCommand::RemoveGlobalEnvVar { name },
        OperationKind::EnvRemove,
    )
    .await
}

/// The version a set is at after a change, as both kinds of save answer it.
#[derive(Serialize)]
struct SetVersion {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

/// Run a whole-set save: `200 {"version"}`, `409 {"error":"changed",
/// "message"}` when the set moved since the version it was based on, `409`
/// while another change holds the set, and `400` with the reason otherwise.
async fn save_whole_set(state: &AppState, headers: &HeaderMap, cmd: WireCommand) -> Response {
    match state
        .engine
        .apply_wire_scoped(cmd, scope_from_headers(headers, &state.connections))
        .await
    {
        Ok(outcome) => Json(SetVersion {
            version: outcome.version,
        })
        .into_response(),
        Err(e) => config_refusal(e),
    }
}

/// Run a one-entry change. With `?operation=1` it answers its operation
/// record, which the change finishes inside the call; without, `200
/// {"version"}` as a whole-set save does.
async fn change_one_entry(
    state: &AppState,
    headers: &HeaderMap,
    operation: &OperationQuery,
    cmd: WireCommand,
    kind: OperationKind,
) -> Response {
    let scope = scope_from_headers(headers, &state.connections);
    if operation.asked() {
        return match state.engine.apply_wire_operation(cmd, scope, kind).await {
            Ok((_, record)) => operation_accepted(&record),
            Err(e) => config_refusal(e),
        };
    }
    match state.engine.apply_wire_recorded(cmd, scope, kind).await {
        Ok(outcome) => Json(SetVersion {
            version: outcome.version,
        })
        .into_response(),
        Err(e) => config_refusal(e),
    }
}

/// How a macro or environment change's refusal answers.
fn config_refusal(error: String) -> Response {
    if dux_core::wire::is_stale_set(&error) {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "changed", "message": error })),
        )
            .into_response();
    }
    if error.starts_with("unknown macro") || error.starts_with("unknown global environment") {
        return (StatusCode::NOT_FOUND, error).into_response();
    }
    refusal(error, StatusCode::BAD_REQUEST)
}

// ── Config-file listings ─────────────────────────────────────────────────────

/// Build a listing from this dux's own `config.toml` off the async runtime:
/// `200` with its JSON array, or `500` with the sentence saying why the file
/// could not be read.
async fn config_listing<T: Serialize + Send + 'static>(
    state: &AppState,
    build: impl FnOnce(&dux_core::config::DuxPaths) -> Result<T, String> + Send + 'static,
) -> Response {
    let paths = state.engine.paths();
    match tokio::task::spawn_blocking(move || build(&paths)).await {
        Ok(Ok(listing)) => Json(listing).into_response(),
        Ok(Err(error)) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "reading config.toml stopped before it finished",
        )
            .into_response(),
    }
}

/// `GET /api/v1/config/providers`: every provider, dux's own included, as
/// `dux providers ls` lists them.
async fn list_providers(State(state): State<AppState>) -> Response {
    config_listing(&state, |paths| {
        dux_core::config_resources::read(&paths.config_path)
            .map(|file| dux_core::config_resources::providers(&file))
    })
    .await
}

/// `GET /api/v1/config/keys`: every action the terminal UI binds, with its
/// keys, as `dux keys ls` lists them.
async fn list_keys(State(state): State<AppState>) -> Response {
    config_listing(&state, |paths| {
        let file = dux_core::config_resources::read(&paths.config_path)?;
        dux_core::config_resources::keys(&file.raw)
    })
    .await
}

/// `GET /api/v1/config/themes`: the themes the terminal UI's picker offers,
/// in its order, as `dux themes ls` lists them.
async fn list_themes(State(state): State<AppState>) -> Response {
    config_listing(&state, |paths| {
        let file = dux_core::config_resources::read(&paths.config_path)?;
        Ok(dux_core::config_resources::themes(
            paths,
            &file.config.ui.theme,
        ))
    })
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
/// so no `Json` extractor is used. A config reload re-reads `config.toml` from disk.
///
/// With `?operation=1` it answers `202` and an operation record that ends when
/// the reload's owner says how it went, which is how `dux config set` learns
/// whether its change took effect. Without it, a bare `200` once the reload is
/// under way.
async fn reload_config(
    State(state): State<AppState>,
    Query(operation): Query<OperationQuery>,
    headers: HeaderMap,
) -> Response {
    if !operation.asked() {
        return dispatch(&state, &headers, WireCommand::ReloadConfig {}).await;
    }
    let scope = scope_from_headers(&headers, &state.connections);
    match state
        .engine
        .apply_wire_operation(
            WireCommand::ReloadConfig {},
            scope,
            OperationKind::ConfigReload,
        )
        .await
    {
        Ok((_, record)) => operation_accepted(&record),
        Err(e) => config_refusal(e),
    }
}

// Each preference toggle below is a parameterless POST: the server owns the
// current value and flips it, so two surfaces never disagree about the next state.

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

/// `POST /api/v1/github/recheck`. Ask `gh` again right now. Writes no config;
/// the reply is the routed status, and a change in availability separately
/// nudges every client to refetch its bootstrap document.
async fn recheck_github(State(state): State<AppState>, headers: HeaderMap) -> Response {
    dispatch(&state, &headers, WireCommand::RecheckGithub {}).await
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
/// body (`{"favicon":"amber"}`) or an empty body (`{}`) both deserialize: the
/// handler only touches the fields that are present, and an empty body is a no-op.
#[derive(Deserialize, Default)]
struct InstanceIdentityBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    favicon: Option<String>,
}

/// `POST /api/v1/config/instance-identity`. Persist this instance's browser tab
/// title (`config.server.title`) and favicon color (`config.server.favicon`). Bare
/// `200`, or plain-text `400` when the engine rejects an unknown favicon color.
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

// The settings-PATCH body nests typed sub-structs by config section, each
// `#[serde(default, deny_unknown_fields)]` with `Option<T>` fields: a flat map
// could enforce neither per-field types nor unknown-key rejection, and flat
// dotted-rename keys fight `deny_unknown_fields` across two groups.

/// The `[ui]` half of a settings-PATCH body. Every field is optional; an
/// absent field is left untouched. Unknown fields are rejected (400) so a
/// typo or a client/server drift surfaces immediately instead of silently
/// no-opping.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct UiSettingsPatch {
    copy_on_select: Option<bool>,
    compose_bar: Option<String>,
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
    tab_reaches_agent: Option<bool>,
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

/// The `[defaults]` half of a settings-PATCH body, shaped like [`UiSettingsPatch`].
/// `provider` is the global default for new agents in projects with no override of
/// their own, validated engine-side against the configured provider list; a
/// project's `default_provider` has its own wire path.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct DefaultsSettingsPatch {
    enable_randomized_pet_name_by_default: Option<bool>,
    provider: Option<String>,
}

/// `PATCH /api/v1/config/settings` body: every group and every leaf optional.
/// `title` and `favicon` stay on `POST /api/v1/config/instance-identity`, and
/// `ui.github_integration` keeps its own endpoint because flipping it arms or
/// disarms background PR syncing.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SettingsBody {
    ui: UiSettingsPatch,
    capabilities: CapabilitiesSettingsPatch,
    defaults: DefaultsSettingsPatch,
    /// Suppress this request's info status. Top-level because it is not a settings
    /// field, and honored by the engine only for a patch confined to the
    /// accessory-bar field, so it can silence no other settings write.
    quiet: bool,
}

/// `PATCH /api/v1/config/settings`. Set explicit values for the Settings modal's
/// fields in one request; an omitted field is left untouched and an empty body is
/// a no-op `200`. A validation error is a plain-text `400` and mutates nothing.
/// The engine clamps numeric fields server-side, so a client's own bounds are
/// UX-only and the saved value comes from the post-save bootstrap refetch.
async fn set_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Result<Json<SettingsBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // Deliberately unlike `set_instance_identity`, which lets axum answer a bad
    // body with its default 422: this route's nested `deny_unknown_fields` structs
    // turn a client typo or field-set drift into a deserialize rejection, so it is
    // mapped to the same plain-text 400 its own validation failures use and a
    // caller need only branch on ok versus 4xx-with-a-message.
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
            mobile_accessory_bar: body.ui.mobile_accessory_bar,
            upload_write_gitignore: body.ui.upload_write_gitignore,
            upload_pasted_text_chars: body.ui.upload_pasted_text_chars,
            auto_reopen_agents: body.ui.auto_reopen_agents,
            show_changes_pane: body.ui.show_changes_pane,
            web_notifications: body.capabilities.web_notifications,
            always_show_tab_strip: body.ui.always_show_tab_strip,
            tab_reaches_agent: body.ui.tab_reaches_agent,
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
    /// Proof of what was read, which the save must send back: the save is
    /// refused when the file changed since.
    token: String,
}

#[derive(Deserialize)]
struct WriteRawConfigBody {
    content: String,
    /// The token the read handed out. A save without one is refused.
    #[serde(default)]
    token: Option<String>,
}

/// `GET /api/v1/config/raw`. Return the raw `config.toml` text for the editor. A
/// read failure, or a missing engine, is a `503` so the editor surfaces an error
/// instead of opening on blank content.
async fn read_raw_config(State(state): State<AppState>) -> Response {
    match state.engine.read_raw_config().await {
        Ok(raw) => Json(RawConfigBody {
            content: raw.content,
            token: raw.token,
        })
        .into_response(),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, e).into_response(),
    }
}

/// `PUT /api/v1/config/raw`. Validate and write the raw `config.toml` text
/// verbatim, over the file the editor read: `409 {error:"config_changed",
/// message}` when the file changed since that read (nothing written; the
/// editor offers to reload or keep editing), and a plain-text `400` with the
/// reason for anything else, a missing token included. Persists only: the
/// running config is untouched and no `config.changed` fires until
/// `POST /api/v1/config/reload`, which is the single apply point.
async fn write_raw_config(
    State(state): State<AppState>,
    Json(body): Json<WriteRawConfigBody>,
) -> Response {
    match state
        .engine
        .write_raw_config(body.content, body.token)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(crate::engine_actor::RawWriteError::Changed(message)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "config_changed", "message": message })),
        )
            .into_response(),
        Err(crate::engine_actor::RawWriteError::Refused(message)) => {
            (StatusCode::BAD_REQUEST, message).into_response()
        }
    }
}

// ── The live Tailscale mode ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct TailscaleModeBody {
    /// One of `auto` | `yes` | `no`. Validated by the engine, which refuses
    /// anything else rather than degrading it.
    mode: String,
}

/// What the mode change did, for the browser to raise as a toast. The sentence
/// travels rather than being rebuilt client-side: the terminal UI shows the same
/// one, and a second copy in TypeScript is how the two drift apart.
#[derive(Serialize)]
struct TailscaleModeReply {
    /// The mode that was saved, canonicalized.
    mode: String,
    /// Whether the sentence is a warning rather than plain information.
    warning: bool,
    message: String,
    /// The parts `message` was built from, in the browser's `Prose` shape, so
    /// the toast it raises draws the address as a chip. Absent for a plain one.
    #[serde(skip_serializing_if = "Option::is_none")]
    segments: Option<Vec<dux_core::prose::ProseSegment>>,
}

/// `POST /api/v1/server/tailscale-mode`. Save `[server] tailscale`, then apply it
/// to the running listener: the write comes first so the choice survives whatever
/// happens to the listener, and the reply says so when nothing is serving.
///
/// A browser on the Tailscale leg choosing `no` cuts its own connection; the reply
/// is written before the unbind lands so this response still arrives.
async fn set_tailscale_mode(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TailscaleModeBody>,
) -> Response {
    if let Err(e) = state
        .engine
        .apply_wire_scoped(
            WireCommand::SetTailscaleMode {
                mode: body.mode.clone(),
            },
            scope_from_headers(&headers, &state.connections),
        )
        .await
    {
        return (StatusCode::BAD_REQUEST, e).into_response();
    }
    // Parsing again rather than threading the engine's answer back: the engine
    // has already refused anything outside the tri-state, so this cannot fail,
    // and it keeps the reply's `mode` canonical.
    let mode = dux_core::config::TailscaleMode::parse(&body.mode).unwrap_or_default();
    let outcome = match state.tailscale_mode.as_ref() {
        Some(control) => control.set_mode(mode).await,
        None => dux_core::config::TailscaleModeOutcome::NotServing,
    };
    let report = outcome.report(mode);
    let (message, segments) = report.message.into_parts();
    Json(TailscaleModeReply {
        mode: mode.as_str().to_string(),
        warning: report.warning,
        message,
        segments,
    })
    .into_response()
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
    async fn the_config_listings_read_this_duxs_own_file_in_the_pickers_order() {
        let (tmp, app) = router_no_auth();
        std::fs::create_dir_all(tmp.path().join("themes")).unwrap();
        std::fs::write(tmp.path().join("themes/alpha.toml"), "").unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[ui]\ntheme = \"alpha\"\n\n[providers.mytool]\ncommand = \"my-cli\"\n",
        )
        .unwrap();
        let get = |uri: &'static str| {
            let app = app.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::builder()
                            .uri(uri)
                            .body(axum::body::Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK, "{uri}");
                let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };
        let themes = get("/api/v1/config/themes").await;
        assert_eq!(
            themes.as_array().unwrap()[..3],
            [
                serde_json::json!({"name": "dux_dark", "source": "bundled", "current": false}),
                serde_json::json!({"name": "alpha", "source": "user", "current": true}),
                serde_json::json!({"name": "ayu_dark", "source": "opaline", "current": false}),
            ]
        );
        let providers = get("/api/v1/config/providers").await;
        assert_eq!(providers[0]["name"], "mytool");
        assert_eq!(providers[0]["source"], "yours");
        assert_eq!(providers[0]["settings"]["command"], "my-cli");
        assert_eq!(providers[1]["name"], "claude");
        assert_eq!(providers[1]["source"], "built in");
    }

    #[tokio::test]
    async fn setting_the_tailscale_mode_saves_it_and_says_it_applies_when_a_listener_starts() {
        // Nothing is serving behind a test router, which is the honest half of
        // the answer: the choice is saved, and the listener half happens later.
        let (_tmp, app) = router_no_auth();
        let resp = app
            .clone()
            .oneshot(json_req(
                "POST",
                "/api/v1/server/tailscale-mode",
                r#"{"mode":"no"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reply["mode"], "no");
        assert_eq!(reply["warning"], false);
        assert!(
            reply["message"]
                .as_str()
                .expect("a sentence")
                .contains("applies when a listener starts"),
            "{reply}"
        );

        // And the write really happened: the next bootstrap carries it.
        let boot = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/bootstrap")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let boot = axum::body::to_bytes(boot.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let boot: serde_json::Value = serde_json::from_slice(&boot).unwrap();
        assert_eq!(boot["tailscale_mode"], "no");
        assert_eq!(
            boot["tailscale_forced_no"], false,
            "a test router is not a --no-tailscale run"
        );
    }

    /// A router with a serve behind its Tailscale route: a stub loop that
    /// records every mode it is asked for and answers each with `answer`. The
    /// real loop's decisions are covered where the loop lives; what a route test
    /// needs is that the request reaches the serve and its answer reaches the
    /// reply body.
    fn tailscale_router(
        answer: dux_core::config::TailscaleModeOutcome,
        forced_no: bool,
    ) -> (
        dux_core::test_scratch::ScratchDir,
        axum::Router,
        std::sync::Arc<std::sync::Mutex<Vec<dux_core::config::TailscaleMode>>>,
    ) {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let handle = crate::test_support::test_engine_handle(tmp.path());
        let (control, mut requests) = crate::serve_legs::TailscaleModeControl::new(
            tokio::runtime::Handle::current(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                recorder.lock().expect("not poisoned").push(request.mode);
                let _ = request.reply.send(answer);
            }
        });
        let app = crate::server::build_app(
            handle,
            axum::Router::new(),
            crate::server::RouterParams::plain_http()
                .with_tailscale_mode_control(control, forced_no),
        );
        (tmp, app, seen)
    }

    async fn reply_of(app: axum::Router, mode: &str) -> serde_json::Value {
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/server/tailscale-mode",
                &format!(r#"{{"mode":"{mode}"}}"#),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn a_run_started_with_no_tailscale_answers_the_route_with_its_refusal() {
        // The flag outranks the config for as long as the run lasts, so the
        // reply has to say that AND that the choice is saved for the next one.
        let (_tmp, app, seen) = tailscale_router(
            dux_core::config::TailscaleModeOutcome::RefusedForcedNo,
            true,
        );
        let reply = reply_of(app.clone(), "yes").await;
        assert_eq!(reply["mode"], "yes");
        assert_eq!(reply["warning"], true, "{reply}");
        let message = reply["message"].as_str().expect("a sentence");
        assert!(message.contains("--no-tailscale"), "{message}");
        assert!(
            message.contains("saved as \"yes\""),
            "the saved half is the other half of the answer: {message}"
        );
        assert_eq!(
            *seen.lock().expect("not poisoned"),
            vec![dux_core::config::TailscaleMode::Yes],
            "the serve is asked exactly once"
        );

        // The write still happened, and the bootstrap tells the browser why the
        // row it just saved cannot take effect yet.
        let boot = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/bootstrap")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let boot = axum::body::to_bytes(boot.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let boot: serde_json::Value = serde_json::from_slice(&boot).unwrap();
        assert_eq!(boot["tailscale_mode"], "yes");
        assert_eq!(boot["tailscale_forced_no"], true);
    }

    #[tokio::test]
    async fn the_route_answers_with_what_the_serve_did_to_the_listener() {
        let leg: std::net::SocketAddr = "100.64.0.5:8080".parse().unwrap();
        let (_tmp, app, seen) = tailscale_router(
            dux_core::config::TailscaleModeOutcome::Applied { bound: Some(leg) },
            false,
        );

        let reply = reply_of(app, "yes").await;
        assert_eq!(reply["mode"], "yes");
        assert_eq!(reply["warning"], false, "{reply}");
        assert!(
            reply["message"]
                .as_str()
                .expect("a sentence")
                .contains("100.64.0.5:8080"),
            "the reply names the address the leg landed on: {reply}"
        );
        assert_eq!(
            *seen.lock().expect("not poisoned"),
            vec![dux_core::config::TailscaleMode::Yes]
        );
    }

    #[tokio::test]
    async fn a_tailscale_mode_outside_the_tri_state_is_refused() {
        let (_tmp, app) = router_no_auth();
        let resp = app
            .oneshot(json_req(
                "POST",
                "/api/v1/server/tailscale-mode",
                r#"{"mode":"maybe"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let message = String::from_utf8(body.to_vec()).unwrap();
        assert!(message.contains("maybe"), "{message}");
        assert!(
            message.contains("auto") && message.contains("yes") && message.contains("no"),
            "the refusal must list the valid values: {message}"
        );
    }

    /// Send `req` and answer its status and body, parsed as JSON when it is.
    async fn answer(
        app: &Router,
        req: Request<axum::body::Body>,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, body)
    }

    async fn bootstrap(app: &Router) -> serde_json::Value {
        let req = Request::builder()
            .uri("/api/v1/bootstrap")
            .body(axum::body::Body::empty())
            .unwrap();
        answer(app, req).await.1
    }

    #[tokio::test]
    async fn update_macros_accepts_a_valid_set() {
        let (_tmp, app) = router_no_auth();
        let (status, body) = answer(
            &app,
            json_req(
                "PUT",
                "/api/v1/macros",
                r#"{"entries":[{"name":"greet","text":"hi","surface":"agent"}]}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // The browser saves the list it read, and says which version that was.
        let read_at = bootstrap(&app).await["macros_version"].clone();
        assert_eq!(
            body["version"], read_at,
            "a save answers the version it left"
        );
        // The command line adds a macro in the meantime.
        let (status, _) = answer(
            &app,
            json_req(
                "PUT",
                "/api/v1/macros/deploy",
                r#"{"text":"ship it","surface":"terminal"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let stale = serde_json::json!({
            "entries": [{"name": "greet", "text": "hello", "surface": "agent"}],
            "version": read_at,
        });
        let (status, body) =
            answer(&app, json_req("PUT", "/api/v1/macros", &stale.to_string())).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "changed");
        let names: Vec<serde_json::Value> = bootstrap(&app).await["macros"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].clone())
            .collect();
        assert_eq!(names, vec!["greet", "deploy"], "the added macro survives");
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
        let (status, _) = answer(
            &app,
            json_req("PUT", "/api/v1/global-env", r#"{"env":{"FOO":"bar"}}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let read_at = bootstrap(&app).await["global_env_version"].clone();
        let (status, _) = answer(
            &app,
            json_req("PUT", "/api/v1/global-env/TOKEN", r#"{"value":"abc"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let stale = serde_json::json!({"env": {"FOO": "baz"}, "version": read_at});
        let (status, body) = answer(
            &app,
            json_req("PUT", "/api/v1/global-env", &stale.to_string()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "changed");
        assert_eq!(
            bootstrap(&app).await["global_env"],
            serde_json::json!({"FOO": "bar", "TOKEN": "abc"})
        );
    }

    /// The per-entry routes change one macro or one variable, refuse what is
    /// not there, and answer an operation record when asked, finished inside
    /// the call.
    #[tokio::test]
    async fn the_per_entry_routes_change_one_entry_and_answer_an_operation_when_asked() {
        // A macro written by hand with spaces around its name: a change to
        // another macro leaves it exactly as it is.
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let mut engine = crate::test_support::unstarted_test_engine(tmp.path());
        engine.config.macros.entries.insert(
            " padded ".to_string(),
            dux_core::config::MacroEntry {
                text: "by hand".to_string(),
                surface: dux_core::config::MacroSurface::Agent,
            },
        );
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::router(handle);
        let (status, record) = answer(
            &app,
            json_req(
                "PUT",
                "/api/v1/macros/Review%20it?operation=1",
                r#"{"text":"review this","surface":"agent"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(record["kind"], "macro.set");
        assert_eq!(record["state"], "succeeded");
        assert!(
            record["message"].as_str().unwrap().contains("Review it"),
            "{record}"
        );

        let (status, _) = answer(
            &app,
            json_req(
                "PUT",
                "/api/v1/macros/bad",
                r#"{"text":"","surface":"agent"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "a macro needs text");

        let (status, record) = answer(
            &app,
            json_req("DELETE", "/api/v1/macros/Review%20it?operation=1", ""),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(record["kind"], "macro.remove");
        assert_eq!(record["state"], "succeeded");
        assert_eq!(
            bootstrap(&app).await["macros"],
            serde_json::json!([{"name": " padded ", "text": "by hand", "surface": "agent"}])
        );
        let (status, _) = answer(&app, json_req("DELETE", "/api/v1/macros/Review%20it", "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, record) = answer(
            &app,
            json_req(
                "PUT",
                "/api/v1/global-env/API_KEY?operation=1",
                r#"{"value":"secret"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(record["kind"], "env.set");
        assert_eq!(record["state"], "succeeded");
        assert!(
            !record.to_string().contains("secret"),
            "a value is never echoed: {record}"
        );
        let (status, refusal) = answer(
            &app,
            json_req("PUT", "/api/v1/global-env/1BAD", r#"{"value":"x"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "not a variable name");
        assert!(
            !refusal.to_string().contains("1BAD"),
            "a name that is no variable name is never echoed: {refusal}"
        );
        let (status, refusal) =
            answer(&app, json_req("DELETE", "/api/v1/global-env/zz%20LEAK", "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(!refusal.to_string().contains("LEAK"), "{refusal}");
        for bad in ["nul\u{0}inside", "${1UNCLOSED}"] {
            let body = serde_json::json!({ "value": bad }).to_string();
            let (status, refusal) =
                answer(&app, json_req("PUT", "/api/v1/global-env/BAD_VALUE", &body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {refusal}");
            assert!(
                !refusal.to_string().contains("inside")
                    && !refusal.to_string().contains("UNCLOSED"),
                "a refused value is never echoed: {refusal}"
            );
        }
        assert!(
            bootstrap(&app).await["global_env"]
                .get("BAD_VALUE")
                .is_none(),
            "a refused value is not kept"
        );
        let (status, record) = answer(
            &app,
            json_req("DELETE", "/api/v1/global-env/API_KEY?operation=1", ""),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(record["kind"], "env.remove");
        assert_eq!(bootstrap(&app).await["global_env"], serde_json::json!({}));
        let (status, _) = answer(&app, json_req("DELETE", "/api/v1/global-env/API_KEY", "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
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

    /// Asked as an operation, a reload answers its record at once and the
    /// record ends on how the reload really went: applied when the file
    /// reads, refused, with the reason, when it does not.
    #[tokio::test]
    async fn a_reload_asked_as_an_operation_ends_on_how_it_went() {
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let engine = crate::test_support::unstarted_test_engine(tmp.path());
        let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
        let app = crate::server::router(handle);

        let (status, record) = answer(
            &app,
            json_req("POST", "/api/v1/config/reload?operation=1", "{}"),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(record["kind"], "config.reload");
        let id = record["id"].as_str().expect("an id").to_string();
        let (status, done) = answer(
            &app,
            json_req(
                "GET",
                &format!("/api/v1/operations/{id}?wait_seconds=10"),
                "",
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(done["state"], "failed", "{done}");
        assert!(
            done["message"]
                .as_str()
                .unwrap()
                .starts_with("Config reload failed:"),
            "{done}"
        );

        std::fs::write(
            tmp.path().join("config.toml"),
            "[ui]\nleft_width_pct = 30\n",
        )
        .unwrap();
        let (_, record) = answer(
            &app,
            json_req("POST", "/api/v1/config/reload?operation=1", "{}"),
        )
        .await;
        let id = record["id"].as_str().expect("an id").to_string();
        let (_, done) = answer(
            &app,
            json_req(
                "GET",
                &format!("/api/v1/operations/{id}?wait_seconds=10"),
                "",
            ),
        )
        .await;
        assert_eq!(done["state"], "succeeded", "{done}");
        assert_eq!(
            done["message"],
            "Configuration reloaded. New settings are active now."
        );
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
        let token = parsed["token"].as_str().expect("a token").to_string();
        let body = serde_json::json!({ "content": content, "token": token }).to_string();
        let put = app
            .oneshot(json_req("PUT", "/api/v1/config/raw", &body))
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
    }

    /// The raw editor writes whatever an authenticated (or unprotected) page
    /// sends, so it is where a password could be swapped or dropped without
    /// the current one. It refuses every change to the credential, and lets
    /// everything else through, the rest of `[server.auth]` included.
    #[tokio::test]
    async fn the_raw_editor_cannot_add_change_or_remove_the_password() {
        let hash = |p: &str| {
            dux_core::auth::hash_password(&dux_core::auth::Password::new(p.to_string())).unwrap()
        };
        // From this machine, which the default `require` asks for nothing: the
        // editor is open to it, and this is about what the editor may write.
        let put = |app: Router, content: String| async move {
            let local = axum::extract::ConnectInfo(crate::auth::Arrival::Tcp {
                peer: "127.0.0.1:40000".parse().unwrap(),
                local: "127.0.0.1:3890".parse().unwrap(),
            });
            let token = raw_token_from(&app, Some(local)).await;
            let body = serde_json::json!({ "content": content, "token": token }).to_string();
            let mut request = json_req("PUT", "/api/v1/config/raw", &body);
            request.extensions_mut().insert(local);
            let answer = app.oneshot(request).await.unwrap();
            let status = answer.status();
            let text = axum::body::to_bytes(answer.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, String::from_utf8_lossy(&text).into_owned())
        };

        // No password yet: adding one through the editor is refused.
        let (tmp, app) = router_no_auth();
        let added = format!(
            "[server.auth]\npassword_hash = \"{}\"\n",
            hash("orbit velvet quarry lantern cobalt")
        );
        let (status, said) = put(app.clone(), added).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{said}");
        assert!(said.contains("password"), "{said}");
        let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap_or_default();
        assert!(!on_disk.contains("$argon2id$"), "{on_disk}");

        // A password set: changing it and removing it are refused; an edit that
        // keeps it, even one tightening the rest of the section, is not.
        let tmp = dux_core::test_scratch::ScratchDir::new();
        let current = hash("orbit velvet quarry lantern cobalt");
        std::fs::write(
            tmp.path().join("config.toml"),
            format!("[server.auth]\npassword_hash = \"{current}\"\n"),
        )
        .unwrap();
        let app = crate::server::router(crate::test_support::test_engine_handle(tmp.path()));
        let other = hash("harbor cinnamon glacier tundra mosaic");
        for (content, what) in [
            (
                format!("[server.auth]\npassword_hash = \"{other}\"\n"),
                "changed",
            ),
            (
                "[server.auth]\npassword_hash = \"\"\n".to_string(),
                "cleared",
            ),
            ("[ui]\n".to_string(), "section removed"),
        ] {
            let (status, said) = put(app.clone(), content).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {said}");
            assert!(said.contains("current password"), "{what}: {said}");
            let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
            assert!(on_disk.contains(&current), "{what}: nothing was written");
        }
        let kept =
            format!("[server.auth]\npassword_hash = \"{current}\"\nrequire = \"everywhere\"\n");
        let (status, said) = put(app, kept).await;
        assert_eq!(status, StatusCode::OK, "{said}");
        let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap();
        assert!(on_disk.contains("require = \"everywhere\""), "{on_disk}");
    }

    /// The structured settings write has no field for anything in
    /// `[server.auth]`, and a body that names one is refused whole, so the
    /// password can never ride in on a settings patch.
    #[tokio::test]
    async fn a_settings_patch_cannot_reach_the_password() {
        let (tmp, app) = router_no_auth();
        for body in [
            serde_json::json!({ "server": { "auth": { "password_hash": "$argon2id$x" } } }),
            serde_json::json!({ "ui": { "password_hash": "$argon2id$x" } }),
            serde_json::json!({ "auth": { "password": "orbit velvet quarry lantern" } }),
        ] {
            let answer = app
                .clone()
                .oneshot(json_req(
                    "PATCH",
                    "/api/v1/config/settings",
                    &body.to_string(),
                ))
                .await
                .unwrap();
            assert_eq!(answer.status(), StatusCode::BAD_REQUEST, "{body}");
        }
        let on_disk = std::fs::read_to_string(tmp.path().join("config.toml")).unwrap_or_default();
        assert!(!on_disk.contains("$argon2id$"), "{on_disk}");
    }

    /// The token the raw read hands out, read as the editor reads it.
    async fn raw_token_from(
        app: &Router,
        from: Option<axum::extract::ConnectInfo<crate::auth::Arrival>>,
    ) -> String {
        let mut request = Request::builder()
            .method("GET")
            .uri("/api/v1/config/raw")
            .body(axum::body::Body::empty())
            .unwrap();
        if let Some(from) = from {
            request.extensions_mut().insert(from);
        }
        let answer = app.clone().oneshot(request).await.unwrap();
        let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        parsed["token"].as_str().expect("a token").to_string()
    }

    /// A save over a file that moved since it was read is a conflict the
    /// browser can tell apart, and writes nothing; one with no token is
    /// refused.
    #[tokio::test]
    async fn a_raw_save_over_a_changed_file_is_a_409_and_one_without_a_token_a_400() {
        let (tmp, app) = router_no_auth();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[ui]\nleft_width_pct = 20\n").unwrap();
        let token = raw_token_from(&app, None).await;
        std::fs::write(&path, "[ui]\nleft_width_pct = 22\n").unwrap();
        let body = serde_json::json!({ "content": "[ui]\nleft_width_pct = 25\n", "token": token });
        let answer = app
            .clone()
            .oneshot(json_req("PUT", "/api/v1/config/raw", &body.to_string()))
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(answer.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed["error"], "config_changed");
        assert!(
            parsed["message"]
                .as_str()
                .unwrap()
                .contains("nothing was saved")
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("= 22"));

        let no_token = serde_json::json!({ "content": "[ui]\nleft_width_pct = 25\n" });
        let answer = app
            .oneshot(json_req("PUT", "/api/v1/config/raw", &no_token.to_string()))
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::BAD_REQUEST);
        assert!(std::fs::read_to_string(&path).unwrap().contains("= 22"));
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
    /// settings field) and still persists the accessory-bar write; the engine
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
                r#"{"ui":{"mobile_accessory_bar":false},"quiet":true}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let raw = read_raw_config_text(&app).await;
        assert!(raw.contains("mobile_accessory_bar = false"), "raw: {raw}");
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
                        "compose_bar": "never",
                        "mobile_accessory_bar": false,
                        "upload_write_gitignore": false,
                        "auto_reopen_agents": true,
                        "always_show_tab_strip": true,
                        "tab_reaches_agent": true,
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
            "compose_bar = \"never\"",
            "mobile_accessory_bar = false",
            "upload_write_gitignore = false",
            "auto_reopen_agents = true",
            "always_show_tab_strip = true",
            "tab_reaches_agent = true",
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
