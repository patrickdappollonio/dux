# Task 3 report: Remove the auth/users subsystem and collapse the server bind model

Status: DONE. All four workspace gates pass.

## dux-core

`crates/dux-core/src/config.rs`
- Deleted `AuthConfig` struct and `Config.auth` field (+ `Config::default()` init).
- `ServerConfig`: removed `listen_addrs`, `bind`, `insecure_allow_remote`,
  `dangerously_listen_http`; added `host: String` and `allowed_hosts: Vec<String>`
  (with documented comments). Rewrote the hand-written `impl Default for ServerConfig`.
- `ServerPlan` collapsed from an enum to `pub struct ServerPlan { pub addrs: Vec<PlanAddr> }`.
- `ServerCliOverrides` reshaped to `{ bind: Option<String>, port: Option<u16>, no_tailscale: bool }`.
- Rewrote `resolve_server_plan` to the 3-arg local-first form (no auth gate, no
  public-bind refusal); added `pub(crate) fn plan_addrs`; rewrote `local_addrs` as a
  thin wrapper over `plan_addrs`.
- Replaced the `resolve_server_plan_tests` module with the new `resolve_plan_tests`
  (9 tests, transcribed from the brief); kept `local_addrs_tests`.
- Updated the bottom config round-trip tests (server defaults/full/partial; deleted
  the deprecated-bind and `[auth]` parse tests).
- `lib.rs`: removed `pub mod auth;`. Deleted `auth.rs`.
- `bcrypt` removed from `crates/dux-core/Cargo.toml`.

Engine reload-barrier removals (the subtle cross-cutting part):
- `engine/mod.rs`: deleted the `pending_auth_users` field and its `AuthUserFinalOutcome`
  re-export.
- `engine/events.rs`: deleted `AuthUserFinalOutcome` enum + impl, `EventReaction::AuthUsersOutcome`
  (+ its label arm), the `pending_auth_users` reload-replay block in `process_config_reload_ready`
  (`must_preswap` is now just `has_deferred`), the `WorkerEvent::AuthUsersPersisted` match arm,
  every `self.config.auth.users` access, and the whole `AuthUsersPersisted` reload-barrier test
  submodule.
- `engine/in_flight.rs`: deleted `InFlightKey::AuthUsers`.
- `engine/test_support.rs`: dropped the `pending_auth_users: None` Engine initializer.
- `worker.rs`: deleted `WorkerEvent::AuthUsersPersisted`.
- `wire.rs`: deleted both `EventReaction::AuthUsersOutcome` match arms.
- `action.rs`: deleted `Action::ServerAddUser`/`ServerRemoveUser` and all four match arms.
- `palette.rs`: deleted the `server-add-user`/`server-remove-user` entries.
- `config_write.rs`: dropped the `[auth] users` patch + the `listen_addrs`/`insecure`/`dangerously`/`bind`
  patch lines; added `host` + `allowed_hosts` patches; replaced the auth round-trip tests with
  `write_config_plain_round_trips_host_and_allowed_hosts`; updated the 0600-perms doc comment.

## dux-web

`build_app` signature change: `(engine, auth, extra_gated, params) -> (Router, SweepableMemoryStore)`
became `(engine, extra_gated, params) -> Router`. Callers updated:
- `lib.rs` `run_plain_http` and `serve_with_engine` (both now `let app = ...`, no sweep handle).
- `crates/dux-web/tests/changes_events.rs` (dropped the `.0` and the `shared_auth` arg).
- All in-module server.rs test callers.

- `server.rs`: deleted the session-manager layer, the `gate` middleware + `route_layer`, the
  `/api/login`/`/api/logout`/`/api/me` registrations, the OPEN-vs-GATED split (all routes plain),
  `router_with_auth`, `build_router_with_recheck`, `build_app_with_store`, `ws_recheck_user`,
  `WS_RECHECK_PERIOD`, and `RouterParams.ws_recheck_period`. Removed the `auth`/`rate_limiter`/
  `ws_recheck_period` `AppState` fields. Dropped the `auth::is_enabled` rechecks and the whole
  user-revocation recheck machinery from both WS socket loops (`handle_pty_socket`,
  `handle_events_socket`) and the upgrade handlers (which no longer take a `Session`). **Kept all
  three `same_origin_allowed` checks.** Deleted the auth-gate tests (FaultOnDeleteStore + refund,
  `gated_data_route_is_401`, `changes_and_events_routes_require_session`, `git/file/rest_action
  _requires_session`); converted the rest to the no-auth `build_app(handle, Router::new(), …)` form.
- `host_guard.rs`: deleted `SweepableMemoryStore`/`spawn_session_sweep`/`SESSION_SWEEP_PERIOD` and the
  tower-sessions imports + their tests. **Kept `DomainAllowlist`/`host_allowlist_layer`/
  `normalize_host_for_match`** (next task uses them); made the allowlist items `pub(crate)`.
- `engine_actor.rs`: deleted `AuthReloadContext`, the `auth_reload` field on `ActorLoopEnds`, and
  merged `build_actor_channels_with_auth`/`spawn_engine_thread_with_auth` back into
  `build_actor_channels`/`spawn_engine_thread`. Deleted the auth-rebuild block in the reload arm
  (incl. the `ctx.console.reload(...)` calls). In `server_rebind_settings_changed` removed the
  `listen_addrs` comparison and **added `host` + `allowed_hosts`** so a host/allowed-hosts change
  still warns "restart to apply"; updated the drift tests.
- `lib.rs`: `run_server` dropped `disable_auth`; `run_plain_http`/`serve_with_engine` dropped all
  auth wiring, the session sweep, and `resolve_host_only`. Deleted `login_row` and the login parts
  of the banner (`plain_http_banner` no longer takes `disable_auth`/`user_count`); **kept
  `Reachability`/`reachability` (marked `#[allow(dead_code)]`) for Task 5's safety note.**
  `ServeShutdown::new()` now returns just `Self`.
- `console.rs`: removed `LoginRow`, the `Banner.login` field + login render block, and the auth-only
  console methods (`login_ok`/`login_failed`/`login_rate_limited`/`logout`/`reload`); marked the
  `Tone` enum `#[allow(dead_code)]` (Ok/Error tones now have no live emit site). Updated tests.
- `bootstrap.rs`: dropped the `pending_auth_users: None` Engine init.
- `test_support.rs`: deleted `router_with_auth`/`router_with_auth` helpers; simplified `router_no_auth`;
  added `boot_plain_test_server()`.
- The five route modules (`config_routes`, `project_reads`, `browse_routes`, `startup_logs`,
  `session_actions`): dropped the `router_with_auth` import and deleted the single formerly-401
  gated test in each.
- Deleted `crates/dux-web/tests/auth_gate.rs`.
- `Cargo.toml`: removed `tower-sessions` and (now-unused) `async-trait`.

## dux-tui

- Deleted `app/auth_users.rs` and `mod auth_users;`.
- `app/mod.rs`: removed the `pending_auth_ops` field + init, the three `PromptState` variants
  (`ServerAddUserName`/`ServerAddUserPassword`/`ServerRemoveUser`), and the `server-add-user`/
  `server-remove-user` command handlers.
- `app/workers.rs`: removed the `EventReaction::AuthUsersOutcome` handling, `apply_auth_users_outcome`,
  and the `AuthUserFinalOutcome` import.
- `app/render.rs` / `app/input.rs`: removed the three prompt render/input handlers, the
  `render_single_input_dialog` + server-user render helpers, and the server-user input test block.
- `app/sessions.rs` / `app/test_support.rs`: dropped the `pending_auth_ops`/`pending_auth_users` inits.
- `app/text_input.rs`: removed the now-unused `masked` field/`masked()`/`display_text()` + their tests.
- `keybindings.rs`: removed the `ServerAddUser`/`ServerRemoveUser` binding defs.
- `cli.rs`: replaced the server config-diff `listen_addrs`/`insecure_allow_remote` rows with `host`/
  `allowed_hosts`; removed the `[auth]` user-count print.
- `server_screen.rs`: removed `auth_enabled`/`user_count` from `ServerStatusScreen::new`, the `AuthInfo`
  role + login line; the non-loopback no-auth warning now fires purely on `!loopback`. Updated tests.
- `config.rs`:
  - Renderer: rewrote the `[server]` section (added `host`, `allowed_hosts`; dropped `listen_addrs`/
    `insecure_allow_remote`/`dangerously_listen_http`); removed the `[auth]` section + `render_auth_config`
    + the `ConfigEntry::Auth` variant.
  - **D-MIGRATE**: rewrote `migrate_server_bind` so a non-loopback `bind` migrates to `host`+`port`
    (with a `logger::warn`) and a loopback `bind` is dropped silently; replaced the two old
    bind-migration tests.
  - **D-HOSTVALIDATE**: added `validate_server_host` (an `IpAddr` parse check) to `ensure_config`
    with reject/accept tests.
  - Added `rendered_server_section_is_local_only`; updated `default_config_is_commented_and_complete`.

## dux binary

`crates/dux/src/main.rs`:
- Reduced `SERVER_USAGE`, `ServerArgs`, `parse_server_args`, `into_overrides` to `{ bind, port,
  no_tailscale }`; `--bind` may be given only once (clear error otherwise); removed `--listen`,
  `--disable-auth`, `--insecure-allow-remote`, `--dangerously-listen-http`, and the old `--bind`
  push semantics.
- Serve path calls `resolve_server_plan(server, cli, tailscale_ip)` (3 args) and
  `dux_web::run_server(paths, plan, version)` (no `disable_auth`); the public-bind warning is now a
  plain no-gate notice. `ServerPlan` read as a struct (`plan.addrs`).
- Flip path drops the `auth_enabled`/`user_count` computation and the corresponding
  `ServerStatusScreen::new` args.
- Rewrote the CLI parse tests (`bind_parses_once`, `second_bind_is_rejected`, `removed_flags_unknown`).

## Scope boundary honored
Added the `allowed_hosts` config field + renderer + round-trip + the `server_rebind_settings_changed`
comparison only. Did NOT wire `allowed_hosts` into the host-guard layer logic (next task).

## Dependency removals
- `bcrypt` removed from `crates/dux-core/Cargo.toml` AND root `Cargo.toml`.
- `tower-sessions` removed from `crates/dux-web/Cargo.toml` AND root `Cargo.toml`.
- `async-trait` removed from `crates/dux-web/Cargo.toml` (its only users — the sweepable store and a
  login test — are gone).
- Confirmed none of bcrypt/tower-sessions/async-trait remain in `Cargo.lock`.

## Verification gate (all four pass)
```
cargo fmt --all                                          -> clean (no output)
cargo clippy --all-targets --all-features -- -D warnings -> Finished, 0 warnings
cargo build --workspace                                  -> Finished
cargo test --workspace                                   -> TOTAL passed=1629 failed=0
```

## Stale-symbol sweep (final)
The sweep returns only:
- `crates/dux-tui/src/config.rs` negative assertions (`assert!(!… .contains("listen_addrs"/"insecure_allow_remote"/"dangerously_listen_http"))`) — these intentionally assert the strings are absent.
All other survivors were cleaned to prose that no longer names removed fields.

## Concerns
- `Reachability`/`reachability` in `dux-web/src/lib.rs` are retained behind `#[allow(dead_code)]`
  per the brief for Task 5's safety note; remove the allow when Task 5 wires them in.
- `Tone::Ok`/`Tone::Error` in `console.rs` now have no live emit site (kept behind `#[allow(dead_code)]`
  as a stable vocabulary).
- `reqwest` dev-dep still carries the `cookies` feature (was for auth tests); left as-is since two
  WS/changes integration tests still use reqwest and removing a feature flag was out of scope.

## Em-dash fix (commit 830343a)

Code review found 19 new em-dashes introduced by commit `b4c63bf`. After
scoping each hit against the base (`34816ed`), 9 were confirmed genuinely new
prose and 10 were verified as relocated pre-existing em-dashes.

### Replaced (9 new em-dashes):

| File | Before fragment | After fragment |
|------|----------------|----------------|
| `crates/dux-core/src/config.rs` | `refusal — the operator` | `refusal; the operator` |
| `crates/dux-tui/src/app/mod.rs` | `at all — this test` | `at all; this test` |
| `crates/dux-tui/src/config.rs` | `dropped silently — the new` | `dropped silently; the new` |
| `crates/dux-tui/src/config.rs` | `"127.0.0.1" — loopback only` | `"127.0.0.1": loopback only` |
| `crates/dux-tui/src/config.rs` | `"0.0.0.0"   — every interface` | `"0.0.0.0":   every interface` |
| `crates/dux-web/src/engine_actor.rs` | `at startup — a` | `at startup; a` |
| `crates/dux-web/src/engine_actor.rs` | `in config — restart` | `in config; restart` |
| `crates/dux-web/src/server.rs` | `Every route — static ... — is served plainly.` | `Every route is served plainly: static ...` (restructured 3 lines) |

### Left as relocated pre-existing content (10 em-dashes):

- `dux-core/src/config.rs`: `` `required: true` — a deliberate listener `` (base line 990)
- `dux-core/src/config_write.rs`: `Unix-only — the project targets` (base line 24)
- `dux-web/src/console.rs`: `The tone of a console line —` (base unchanged)
- `dux-web/src/lib.rs`: `` [`server`] — the axum router `` (base line 18)
- `dux-web/src/lib.rs`: `FATAL — it logs a` (base line 155)
- `dux-web/src/lib.rs`: `first-error wind-down —` (base line 215)
- `dux-web/src/lib.rs`: `supervised poller via \`tokio::spawn\` —` (base lines 753-754)
- `dux-web/src/server.rs`: `Non-browser clients (no \`Origin\`) are allowed —` (base line 23)
- `crates/dux/src/main.rs`: `only — never block.` (base line 171)
- `crates/dux/src/main.rs`: `Tailscale not detected ({}) —` (base)

### Tests updated

None. The `default_config_is_commented_and_complete` test does not assert on
any of the comment text that changed (it asserts only on TOML keys and values).
No other test asserted on the changed strings.

### Gate output

```
cargo fmt --all                          -> clean (no output)
cargo build --workspace                  -> Finished `dev` profile in 1.97s
cargo test -p dux-tui -p dux-core -p dux-web -> all passed (0 failed)
```
