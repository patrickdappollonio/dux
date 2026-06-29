# Local-Only Web Server (remove TLS, ACME, auth, and exposure gating) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce `dux server` to a trusted-local web server that binds a single configurable `host:port` (loopback by default) plus an opportunistic Tailscale leg, with no TLS, no ACME/Let's Encrypt, and no login/auth of any kind.

**Architecture:** Collapse `resolve_server_plan` from a three-mode resolver (ACME / LOCAL / FULL WEB) into one path that returns a list of plain-HTTP `PlanAddr`s from an explicit bind address plus an optional best-effort Tailscale address. Delete the entire ACME/TLS subsystem and the entire auth subsystem. Auth is not a leaf module: it is wired into the dux-core engine's config-reload barrier (a deferred-write machinery), the action/palette/keybinding tables, the wire protocol, the TUI App state, and the React store. All of that comes out together. Preserve and re-home the DNS-rebinding Host-header allowlist and the WebSocket same-origin checks, and add a server-side same-origin check to REST mutations (the cookie that used to provide cross-site-request protection is gone). The web server stays single-tenant/trusted-access; access control is delegated to loopback scope, Tailscale ACLs, or an upstream proxy the operator may add.

**Tech Stack:** Rust (workspace crates: `dux`, `dux-core`, `dux-web`, `dux-tui`), axum 0.8 / tokio for the web server, React + Vite + Tailwind v4 for the web UI.

## Global Constraints

- Platforms are macOS and Linux only. No `#[cfg(windows)]`, no `cfg!(windows)`. Assume Unix.
- No em-dashes anywhere (code comments, strings, docs, commit messages). Use commas, periods, or parentheses.
- Commit messages are plain sentences. No conventional-commit prefixes. No structured trailers.
- Every settable value stays configurable, and the canonical config renderer documents each setting with an inline comment. The config file is the documentation.
- The new `host` and `allowed_hosts` fields are portable user intent and belong in config. Keep runtime/derived state out of config.
- TUI UI styling uses `theme.rs` semantic colors. Web UI styling uses shadcn/base-ui token CSS variables, never hardcoded colors.
- All blocking work (git, file I/O, network, Tailscale detection) runs on background workers, never the main UI thread.
- Never use byte-based slicing/`.len()` to truncate user-visible strings; use `.chars()`.
- The web server remains single-tenant / trusted-access by design. Do not add per-user isolation.

### Verification gate (READ THIS: it changed from the usual per-crate form)

The deletions in this plan remove enum variants, struct fields, and modules that are referenced across **all four Rust crates at once** (a `dux_core` enum variant is matched in `dux-web` and `dux-tui`; `run_server`'s signature is shared by `dux-web` and the `dux` binary). Therefore **a task is "done" only when the whole workspace builds and tests pass**, not when one crate does. Every Rust task ends with:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings   # CI gate; unused imports/params after deletions WILL fail this
cargo build --workspace
cargo test --workspace
```

Web UI tasks end with, inside `crates/dux-web/web/`:

```bash
npm run build && npm test
```

Do not trust `cargo test -p <one-crate>` as a task gate. It can pass while the workspace is red.

### Task sizing note (READ THIS: tasks are feature-coherent, not crate-coherent)

The v1 of this plan tried to split work per-crate and per-file; an adversarial review proved that ordering leaves the workspace uncompilable at almost every task boundary, because removing one cross-crate symbol strands consumers in three other crates. The honest unit of change here is **one feature removed end-to-end across every crate that references it**. That makes a few tasks large. They are large by necessity, not scope creep: you cannot delete `WorkerEvent::AuthUsersPersisted` and keep `dux-core`, `dux-web`, and `dux-tui` green in separate commits. Each large task carries an ordered deletion checklist (consumers before definitions) so it compiles at the end.

### Symbol references use names, not line numbers

The earlier scoping reported many wrong line numbers (off by hundreds of lines). This plan references symbols **by name** and gives a `rg` command to locate them. Verified current locations are noted as "(currently ~N)" but always re-grep; other agents are committing to this branch concurrently and lines will move.

### Standing rule: deleting a symbol means deleting its tests too (READ THIS)

Every type, field, enum variant, function, or module this plan removes is referenced from both **production code** and **test code** (inline `#[cfg(test)]` modules, the `crates/dux-web/tests/` directory, and React components/tests under `crates/dux-web/web/src/`). A deletion that updates the files this plan names but leaves any other consumer fails the gate (a compile error for a removed type/field/export; a runtime assertion failure for a removed rendered string). **For every symbol you delete, run `rg` across the WHOLE tree (all crates, `components/` not just `lib/`, tests not just src) and delete or rewrite every consumer in the same task.** The plan's per-task file lists are a starting point, not an exhaustive enumeration: line numbers drift as other agents commit, and the earlier review passes proved consumers hide in unlisted files (React components, a `RateLimiter` import, a `build_app` return-type caller). The workspace build+test gate is the backstop, but find them proactively with grep.

The high-density test clusters to sweep (re-grep each; these are where deletions strand the most tests):
- `crates/dux-core/src/config.rs` test module: helpers `server`, `server_listen`, `acme_on` (take/return `AcmeSettings`) and ~18 tests calling them; the ACME+auth resolver tests; the `auth_config_*` parse tests. After Task 2/3 these must be deleted or rewritten to the new resolver shape (keep `local_addrs_tests`, add `resolve_plan_tests`).
- `crates/dux-core/src/config_write.rs` test module: `write_config_plain_round_trips_server_section`, `write_config_plain_round_trips_acme_settings`, `patch_preserves_existing_acme_settings`, and the auth-users round-trip tests. Replace with the new host/allowed_hosts round-trip test.
- `crates/dux-web/src/lib.rs` `#[cfg(test)]` module: its `use super::{...}` import line names `acme_banner`, `acme_disable_auth_warning`, `login_row`, `resolve_host_only`; delete those names from the import and delete the ACME-banner tests (Task 2), the `login_row_*` tests, and update the `plain_http_banner_*` tests that pass `disable_auth`/`user_count` args (Task 3).
- `crates/dux-web/tests/`: delete `auth_gate.rs` and `tls_serving.rs`; **update `changes_events.rs`** (it calls `dux_web::auth::shared_auth(&[], false)` and passes it to `build_app`, so it breaks when the auth module and the `build_app` signature change). Re-grep `rg -l 'shared_auth|build_app|router_with_auth|hash_password' crates/dux-web` for any other test file.
- `crates/dux-tui/src/config.rs` test module: `default_config_is_commented_and_complete` (asserts the presence of `[server.acme]`, `[auth]`, `listen_addrs`, `server-add-user`, `--disable-auth` strings, all removed), `server_acme_section_documents_key_concepts`, `rendered_config_with_acme_round_trips`, and the two `migrate_server_bind` tests (which assert on `listen_addrs`/`bind`). Rewrite to the new local-only output (negative assertions that the removed sections are absent + positive assertions for `host`/`allowed_hosts`).
- The five dux-web route modules and `server.rs` in-module tests using `router_with_auth` (see Task 3b).

## End-State Surface

**`[server]` config section:**
- `host: String` (default `"127.0.0.1"`) — NEW. IP `dux server` binds. Loopback keeps it local; `0.0.0.0` exposes on every interface.
- `port: u16` (default `8080`) — kept.
- `tailscale_enabled: bool` (default `true`) — kept.
- `allowed_hosts: Vec<String>` (default `[]`) — NEW. Extra exact `Host`-header values the DNS-rebinding guard accepts. The rendered config comment MUST call out the MagicDNS case explicitly, e.g.: "If you reach this server through Tailscale MagicDNS (like my-box.tailnet-1234.ts.net), add that hostname here; the Tailscale 100.x IP is allowed automatically. Loopback and the IPs dux binds are always allowed." **No `"*"` wildcard** (see Decision D-WILDCARD).
- `color: String`, `access_log: bool`, `max_websocket_connections: u32` — kept unchanged.
- REMOVED: `listen_addrs`, `bind`, `insecure_allow_remote`, `dangerously_listen_http`, `acme` (`[server.acme]` table and `AcmeSettings`).

**`[auth]` section:** REMOVED entirely.

**`dux server` flags:**
- `--bind <ADDR:PORT>` — bind this exact `IP:port`, e.g. `0.0.0.0:8888`. Overrides `[server] host`+`port`. Takes precedence over `--port`. Passing it more than once is an error.
- `--port <PORT>` — override `[server] port` only. Ignored when `--bind` is given.
- `--no-tailscale` — skip the Tailscale leg this run.
- `-h`/`--help`.
- REMOVED: `--listen`, the old repeatable `--bind` alias semantics, `--disable-auth`, `--insecure-allow-remote`, `--acme-domain`, `--acme-email`, `--http-port`, `--https-port`, `--no-acme`, `--dangerously-listen-http`.

**`ServerPlan`:** collapses from a 2-variant enum to `pub struct ServerPlan { pub addrs: Vec<PlanAddr> }`.

**`run_server` signature:** drop the `disable_auth: bool` parameter.

### Decisions locked in by the review

- **D-WILDCARD:** `allowed_hosts` does NOT support a `"*"` accept-anything sentinel. Two reviewers showed it would silently disable both the DNS-rebinding guard and the WebSocket same-origin check at once, even on a `0.0.0.0` bind, with only a config comment as the guardrail. A user needing a MagicDNS or proxy hostname lists it explicitly. If a blanket bypass is ever truly needed, it must be a separate, clearly-named boolean, designed deliberately. Not in this plan.
- **D-ORIGIN:** The three existing WebSocket `same_origin_allowed` checks (in the session-PTY, terminal-PTY, and events upgrade handlers) are KEPT. Removing auth removes the `SameSite` session cookie, which was the cross-site-request protection for REST mutations; to replace it we ADD a server-side same-origin check to REST mutation routes. Both the WS and REST checks compare full `host:port` authority (not host alone) so a different port on the same IP is still cross-origin.
- **D-HOSTVALIDATE:** `host` is validated as an `IpAddr` early, so a typo surfaces before the server runs, per the explicit-failure tenet. It must go in `ensure_config` (`crates/dux-tui/src/config.rs`, which returns `Result<Config>` and runs before the TUI starts), NOT in `dux-core`'s `load_config` (which returns a bare `Config` and falls back to `Config::default()` on error, so it can only log, not reject). `resolve_server_plan` already validates `host` as a final backstop (it returns `Result` and parses with `?`), so a bad host can never reach a running server regardless.
- **D-MIGRATE:** Existing configs carrying the now-removed `bind` key are migrated to `host`/`port` (non-loopback) with a warning, instead of the old code path that rewrites them into the now-removed `listen_addrs` (which serde would silently drop). Removed `[auth]` and `[server.acme]` sections parse-and-ignore harmlessly because `Config` has no `deny_unknown_fields`.

---

## Task 1: Remove the React login UI and auth client (frontend, independent)

This task touches only `crates/dux-web/web/` and is independent of the Rust work; it can run in parallel. Gate: `npm run build && npm test`.

**Files:**
- Delete: `web/src/components/LoginScreen.tsx`, `web/src/lib/auth.ts`, `web/src/lib/auth.test.ts`, `web/src/lib/storeAuth.test.ts`, `web/src/lib/storeAuthRecovery.test.ts`.
- Modify: `web/src/lib/store.ts` — delete the `AuthPhase`/`AuthState` machine, `bootAuth`, `login`, `logout`, `storeAuth`, and the `/api/login|logout|me` calls; boot goes straight to loading the workspace spine and opening the events socket.
- Modify: the mount point that renders `LoginScreen` — render the workspace directly; remove any "log out" control.
- Modify the **production components that consume the auth slice or `logout`** (NOT just `LoginScreen`): `web/src/components/MobileShell.tsx` (imports `logout`; gates a "Log out" button on `auth.phase === "authed"`, reads `auth.username`) and `web/src/components/CommandPalette.tsx` (imports `logout`; a "Log out" palette entry gated on `auth.phase`). Remove the `logout` import, the `auth` destructure from `useDux()`, and the log-out controls.
- Modify the **12 store test files that use `auth.phase` as a boot-settled guard** (NOT auth assertions): `storeChanges.test.ts`, `storeChangesPane.test.ts`, `storeBootstrap.test.ts`, `storeTerminals.test.ts`, `storeStartupLogs.test.ts`, `storeStatusToasts.test.ts`, `restActionsStore.test.ts`, `storeSpineRace.test.ts`, `storeMacros.test.ts`, `storeSpine.test.ts`, `storeDeepLink.test.ts`, `storeCreateFocus.test.ts`.
- **Find ALL consumers (do not trust the lists above) with a tree-wide grep, not a `lib/*.test.ts`-scoped one:** `rg -ln 'auth\.phase|\blogout\b|bootAuth|LoginScreen|/api/(me|login|logout)' crates/dux-web/web/src` and handle every hit (components AND tests).

- [ ] **Step 1: Locate every consumer, tree-wide.** Run `rg -ln 'auth\.phase|\blogout\b|bootAuth|LoginScreen|/api/(me|login|logout)' crates/dux-web/web/src`. Separate the hits into: (a) the two production components (`MobileShell.tsx`, `CommandPalette.tsx`) whose log-out controls get removed, (b) the test files whose `auth.phase` is a boot-settled guard (`await vi.waitFor(() => expect(...getSnapshot().auth.phase).not.toBe("checking"))` — a synchronization helper, not an auth test), and (c) the files deleted outright.

- [ ] **Step 2: Pick the replacement settled-signal** by reading `store.ts` boot: after auth is removed, boot settles when the workspace spine is loaded. Confirm the field name (likely `getSnapshot().spine !== null` or a `ready`/`booted` flag). Use whatever the post-removal boot sets last.

- [ ] **Step 3: Update the 12 files** to wait on the new settled signal instead of `auth.phase`. Delete `LoginScreen.tsx`, `auth.ts`, `auth.test.ts`, `storeAuth.test.ts`, `storeAuthRecovery.test.ts` (the last two are entirely auth-behavior tests with no surviving equivalent). Strip the auth machine from `store.ts` and the mount point.

- [ ] **Step 4: Add a boot test** asserting boot reaches the settled state with no login step and no `/api/me` call (mock fetch; assert `/api/me` is never requested).

- [ ] **Step 5: Run** `cd crates/dux-web/web && npm run build && npm test` (expect PASS).

- [ ] **Step 6: Commit**

```bash
git add -A crates/dux-web/web
git commit -m "Remove the web login screen and auth client"
```

---

## Task 2: Remove ACME and the TLS serving subsystem (Rust, all crates)

Do this BEFORE Task 3. With ACME gone, the resolver still has its plain-HTTP LOCAL and FULL-WEB modes (which still reference auth), so the workspace stays green; Task 3 then removes auth and collapses the resolver. **Re-home the DNS-rebinding allowlist instead of deleting it** (Task 4 generalizes it), because it currently lives inside `tls.rs`.

**Locate the symbols first:**

```bash
rg -n 'rustls_acme|axum_server|AcmeSettings|ServerPlan::Acme|resolve_acme_cache_dir|fn run_acme|acme_banner|acme_disable_auth_warning|fn patch_acme|render_server_acme_config|rebind_drift_detects_acme|console\.acme|DomainAllowlist|host_allowlist_layer|host_allowlist_middleware|spawn_session_sweep|SweepableMemoryStore' crates/
rg -n 'server\.acme|\.acme\b|http_port|https_port|acme_domain|acme_email|no_acme' crates/dux/src/main.rs crates/dux-tui/src
```

**Files (verify each with the grep above; lines drift):**
- `crates/dux-core/src/config.rs` — delete `AcmeSettings` (struct + `Default`), the `acme: AcmeSettings` field on `ServerConfig`, the `ServerPlan::Acme` variant, `resolve_acme_cache_dir`, the ACME branch in `resolve_server_plan`, and the ACME fields on `ServerCliOverrides` (`acme_domains`, `acme_email`, `http_port`, `https_port`, `no_acme`). **The hand-written `impl Default for ServerConfig` has an `acme: AcmeSettings::default()` line — delete that line in THIS task (no replacement; the other deprecated fields stay until Task 3, which does the full Default rewrite). Otherwise Task 2 leaves the Default referencing a deleted type and field and the build gate fails.** After this, `ServerPlan` is still an enum with only `PlainHttp` until Task 3 collapses it to a struct (leave it an enum here to minimize churn).
- `crates/dux-core/src/config_write.rs` — delete `fn patch_acme` (currently ~431) and its call site (currently ~312); delete the ACME round-trip test `write_config_plain_round_trips_acme_settings` (currently ~848).
- `crates/dux-web/src/tls.rs` — **before deleting the file, MOVE `DomainAllowlist`, `host_allowlist_middleware`, and `host_allowlist_layer` into a new module `crates/dux-web/src/host_guard.rs`** (Task 4 generalizes them). Then delete everything else in `tls.rs` (the ACME state, `serve_https_acme`, `serve_http_challenge`, `SweepableMemoryStore`, `spawn_session_sweep`, the ACME event tasks) and the file itself; replace `mod tls;` with `mod host_guard;` in `lib.rs`. Note: `SweepableMemoryStore`/`spawn_session_sweep` are the session store; they are only used by auth, removed in Task 3, but they live here, so delete them now and remove their call sites in `lib.rs` (the `spawn_session_sweep(...)` call, currently ~434) and `server.rs` in this task as part of dropping the session layer is NOT yet possible because the gate still references auth. To keep this task green: the session store deletion forces the session-layer deletion, which forces auth removal. THEREFORE: keep `SweepableMemoryStore`/`spawn_session_sweep` alive by moving them into `host_guard.rs` (or a `sessions.rs`) TEMPORARILY alongside the allowlist, and let Task 3 delete them with the rest of auth. Only the ACME-specific code is deleted in Task 2.
- `crates/dux-web/src/lib.rs` — delete `run_acme`, `acme_banner`, `acme_disable_auth_warning`, `ACME_GRACEFUL_SHUTDOWN`, the `console.acme(...)` calls (currently ~707, ~727, inside `run_acme`), and the `ServerPlan::Acme` match arm in the serve dispatch. The plain-HTTP serve path already uses `axum::serve` (currently ~460); leave it. Drop the now-unused `axum_server::Handle` usage (currently ~684).
- `crates/dux-web/src/console.rs` — delete `fn acme` (currently ~447) if no callers remain after the `lib.rs` deletions.
- `crates/dux-web/src/server.rs` — delete the now-orphaned TLS-only router config: `RouterParams::tls()` (currently ~178, its only caller was `run_acme`), the `secure_cookie` field on `RouterParams` (currently ~146) and its initializers in `plain_http()`/`tls()`, and the `.with_secure(params.secure_cookie)` call (currently ~334). These are `pub`, so the `dead_code` lint will NOT catch them; delete explicitly.
- `crates/dux-web/src/engine_actor.rs` — in `server_rebind_settings_changed` (currently ~242 to ~277) delete the `prev.acme`/`next.acme` comparison; delete the test `rebind_drift_detects_acme_field_changes` (currently ~1953). (The `listen_addrs`/`insecure_allow_remote` comparisons and their drift tests, currently ~1934 and ~1986, are removed in Task 3 when those fields disappear; leave them here.)
- `crates/dux-core/src/statusline.rs` — remove the `"acme"` status key if present.
- `crates/dux/src/main.rs` — delete the ACME flags from `SERVER_USAGE`, `ServerArgs`, `parse_server_args`, and `into_overrides` (`--acme-domain`, `--acme-email`, `--http-port`, `--https-port`, `--no-acme`); delete the `ServerPlan::Acme` match arm after `resolve_server_plan` (currently ~221 to ~254) and the `dux_web::acme_disable_auth_warning()` call inside it (currently ~252).
- `crates/dux-tui/src/config.rs` — delete `fn render_server_acme_config` (currently ~1031), its `ConfigEntry::ServerAcme` match arm (currently ~727), **the `ServerAcme` variant in the `ConfigEntry` enum (currently ~344), and the `ConfigEntry::ServerAcme` entry in `config_schema()` (currently ~665)** (otherwise the match goes non-exhaustive). Also delete the ACME tests per the standing rule.
- `crates/dux-tui/src/cli.rs` — delete the `server.acme.*` diff-output block (currently ~320 to ~364).
- Delete: `crates/dux-web/tests/tls_serving.rs`.
- `Cargo.toml` (workspace) and `crates/dux-web/Cargo.toml` — remove `rustls-acme`, `axum-server`, `rcgen` ONLY after proving no remaining users (Step 5).

- [ ] **Step 1: Move the host allowlist and session-sweep OUT of `tls.rs`.** Create `crates/dux-web/src/host_guard.rs`; move `DomainAllowlist`, `host_allowlist_middleware`, `host_allowlist_layer`, the `normalize_host_for_match`/`strip_host_port` helpers, the `SESSION_SWEEP_PERIOD` const (currently ~707), and (temporarily) `SweepableMemoryStore` + `spawn_session_sweep` into it verbatim. Update the `lib.rs` import (currently `use crate::tls::{AcmePlan, SESSION_SWEEP_PERIOD};` at ~71 — drop `AcmePlan`, point `SESSION_SWEEP_PERIOD` at `host_guard`) and the three `tls::spawn_session_sweep(...)` call sites (currently ~434, ~680, ~1095) to `host_guard::`. Wire `mod host_guard;`. Build to confirm green before deleting anything.

- [ ] **Step 2: Delete the ACME code** listed above, working from leaves inward so each intermediate compiles: CLI flags and TUI renderers, **then `engine_actor.rs` (the `prev.acme`/`next.acme` comparison and the `rebind_drift_detects_acme_field_changes` test) BEFORE the config types**, then `lib.rs` `run_acme`, then the `config.rs` `AcmeSettings`/`ServerPlan::Acme`/resolver branch, then `config_write` and its ACME tests (`write_config_plain_round_trips_acme_settings` AND the adjacent `patch_preserves_existing_acme_settings`), then the `tls.rs` ACME remnants and the file, then `tls_serving.rs`. Per the standing test rule, also delete the dux-tui `config.rs` ACME tests (`server_acme_section_documents_key_concepts`, `rendered_config_with_acme_round_trips`) and the dux-core `config.rs` ACME test helpers/tests, and the `lib.rs` ACME-banner tests (and their names in the test-module `use` line).

- [ ] **Step 3: Add `host` change to the rebind drift check now is premature** (the `host` field does not exist until Task 3). Skip; Task 3 adds it.

- [ ] **Step 4: Prove the TLS deps are unused before removing them.**

```bash
for d in rustls_acme axum_server rcgen; do echo "== $d =="; rg -n "use ${d}|${d}::" crates/ || echo "  none"; done
cargo tree -p dux-web -i rustls 2>/dev/null   # who still needs rustls? (tokio-tungstenite/reqwest test features may)
```

Remove `rustls-acme`, `axum-server`, `rcgen` from both Cargo.toml files. KEEP `rustls` if `cargo tree` shows a surviving reverse-dep (e.g. a websocket/reqwest test client); document the decision in the commit body. If `rustls` is now unused, remove it too.

- [ ] **Step 5: Run the full gate** (fmt, clippy -D warnings, build --workspace, test --workspace). The `rg ... acme|rustls_acme|axum_server` sweep must return nothing outside intended survivors.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "Remove the ACME and TLS serving subsystem and serve plain HTTP only"
```

---

## Task 3: Remove the auth/users subsystem end-to-end and collapse the server bind model (Rust, all crates)

This is the largest task. Auth removal and the resolver/exposure collapse are done together because `resolve_server_plan` takes `auth_enabled`/`auth_explicitly_disabled`, and those inputs vanish with auth, so the exposure gate that consumes them must collapse in the same change. Work the checklist in order (consumers before definitions) so the workspace compiles at the end.

**Locate everything first:**

```bash
rg -n 'mod auth|dux_core::auth|crate::auth|::auth::|AuthConfig|config\.auth|\.auth\.users|auth_enabled|disable_auth|AuthUsersPersisted|AuthUsersOutcome|AuthUserFinalOutcome|pending_auth_users|pending_auth_ops|InFlightKey::AuthUsers|ServerAddUser|ServerRemoveUser|AuthReloadContext|spawn_engine_thread_with_auth|build_actor_channels_with_auth|host_only|same_origin_allowed|router_with_auth|shared_auth|is_enabled\(' crates/
```

### 3a. dux-core: config model + engine reload barrier + actions/palette/wire

**Files (re-grep for current lines):**
- `crates/dux-core/src/config.rs` — delete `AuthConfig`; remove `pub auth: AuthConfig` from `Config` (currently ~845). Add `host: String` and `allowed_hosts: Vec<String>` to `ServerConfig`; remove `listen_addrs`, `bind`, `insecure_allow_remote`, `dangerously_listen_http`. Collapse `ServerPlan` to `pub struct ServerPlan { pub addrs: Vec<PlanAddr> }`. Reshape `ServerCliOverrides` to `{ bind: Option<String>, port: Option<u16>, no_tailscale: bool }`. Rewrite `resolve_server_plan` to the 3-arg local-first form (code below). Add `plan_addrs` and rewrite `local_addrs` as a wrapper (`pub(crate) fn plan_addrs`, since only this crate uses it; `local_addrs` stays `pub` for the TUI flip).
- `crates/dux-core/src/auth.rs` — delete the file; remove `mod auth;` from `lib.rs`.
- `crates/dux-core/src/engine/mod.rs` — delete the `pending_auth_users` field on `Engine` (currently ~216).
- `crates/dux-core/src/engine/events.rs` — delete: `AuthUserFinalOutcome` enum + impl (currently ~413 to ~444), `EventReaction::AuthUsersOutcome` variant (currently ~222 to ~228) and its label arm (currently ~2170), the `pending_auth_users` reload-replay block (currently ~1280, ~1326 to ~1397), the `WorkerEvent::AuthUsersPersisted` match arm (currently ~1961 to ~2006), every `self.config.auth.users` access, and the entire AuthUsersPersisted test submodule (currently ~4001 to ~4194).
- `crates/dux-core/src/engine/in_flight.rs` — delete `InFlightKey::AuthUsers` (currently ~20).
- `crates/dux-core/src/engine/test_support.rs` — delete the `pending_auth_users: None` initializer (currently ~80).
- `crates/dux-core/src/worker.rs` — delete `WorkerEvent::AuthUsersPersisted` (currently ~384).
- `crates/dux-core/src/wire.rs` — delete the `EventReaction::AuthUsersOutcome` match arms (currently ~503, ~524).
- `crates/dux-core/src/action.rs` — delete `Action::ServerAddUser`/`ServerRemoveUser` (currently ~113) AND all their match arms (currently ~212, ~347, ~447).
- `crates/dux-core/src/palette.rs` — delete the `server-add-user`/`server-remove-user` entries.
- `crates/dux-core/src/config_write.rs` — delete the `[auth] users` patch call `patch_table_string_array(doc, "auth", "users", ...)` (currently ~315) and the auth round-trip tests (currently ~914 to ~964); remove the `listen_addrs`/`insecure_allow_remote`/`dangerously_listen_http`/`bind` patch lines; ADD `host` and `allowed_hosts` patches following the existing `patch_*` pattern.

### 3b. dux-web: auth state, sessions, gate, login routes, engine-actor wiring

**Files:**
- `crates/dux-web/src/auth.rs` — delete the file; remove `mod auth;`.
- `crates/dux-web/src/host_guard.rs` (or `sessions.rs`) — delete the temporarily-relocated `SweepableMemoryStore`/`spawn_session_sweep` (no sessions anymore). Keep the host allowlist (Task 4 uses it).
- `crates/dux-web/src/server.rs` — delete the session-manager layer, the `gate()` middleware and its `route_layer`, the `/api/login`/`/api/logout`/`/api/me` registrations, and the OPEN-vs-GATED split (all routes become plain). In the WebSocket upgrade handler, delete the `auth::is_enabled(&state.auth)` recheck (currently ~1146) and set the recheck user to `None` unconditionally. **Keep all three `same_origin_allowed` checks (currently ~751, ~814, ~1139)** per D-ORIGIN. Delete the `RateLimiter` production references (it lives in `auth.rs`): the name in `use crate::auth::{...}` (currently ~44), the `rate_limiter: RateLimiter` field on `AppState` (currently ~59), and its `RateLimiter::default()` initializer (currently ~310). **Change `build_app`'s return type from `(Router, SweepableMemoryStore)` to `Router`** (the session store is gone); update its body to stop building/returning the store. Update the in-module `#[cfg(test)]` tests that call the auth helpers to build the app without auth.
- `build_app` callers (the tuple/`.0` form breaks when the return type collapses): `lib.rs` (currently ~426 `let (app, store) = ...` and ~1081 `let (app, sweep_store) = ...`) become `let app = ...` with the following `spawn_session_sweep` line deleted; `crates/dux-web/tests/changes_events.rs` (currently ~154 `build_app(...).0`) drops the `.0` and its `shared_auth` argument.
- `crates/dux-web/src/test_support.rs` — delete `router_with_auth` and the `dux_core::auth::hash_password`/`auth::shared_auth` calls (currently ~45, ~48); simplify `router_no_auth` to build the router directly. Add a `boot_plain_test_server()` helper (Task 2/3 tests need it; it does not exist yet) that binds `127.0.0.1:0`, serves the no-auth router via `axum::serve` on a `tokio::spawn`, and returns the bound `SocketAddr`.
- The five route modules with gated tests: `config_routes.rs`, `project_reads.rs`, `browse_routes.rs`, `startup_logs.rs`, `session_actions.rs` — convert their formerly-401 gated tests to assert the now-open behavior (200 / handler result) or delete the auth-specific ones.
- `crates/dux-web/src/engine_actor.rs` — delete `AuthReloadContext` (currently ~206 to ~233); merge `spawn_engine_thread_with_auth`/`build_actor_channels_with_auth` back into `spawn_engine_thread`/`build_actor_channels` (drop the auth context param); delete the auth-rebuild block (the `if let Some(ctx) = auth_reload ...` spanning currently ~1026 to ~1081, including the `ctx.console.reload(...)` calls). In `server_rebind_settings_changed` delete the `listen_addrs`/`insecure_allow_remote` comparisons and their drift tests (currently ~1934, ~1986), and ADD `host` and `allowed_hosts` to the comparison so a host change still triggers a restart warning.
- `crates/dux-web/src/lib.rs` — delete `disable_auth` from the `run_server` signature (currently ~97); delete the `AuthReloadContext` construction in `run_plain_http` (currently ~407 to ~415) and in `serve_with_engine` (the `build_actor_channels_with_auth(... AuthReloadContext {...})` currently ~1000 to ~1010); delete `host_only`, `resolve_host_only`, `login_row`, and the login parts of the startup banner (the reachability label survives for Task 5's safety note). Update `build_app` calls that passed `Arc<auth>` (currently ~428).
- `crates/dux-web/src/bootstrap.rs` — delete the `pending_auth_users: None` Engine initializer (currently ~96).
- `crates/dux-web/Cargo.toml` — remove `tower-sessions`; ALSO remove its `[workspace.dependencies]` entry in the root `Cargo.toml` (currently ~35), since no crate uses it after this (mirror how `bcrypt` is removed from both places).
- Delete: `crates/dux-web/tests/auth_gate.rs`.

### 3c. dux-tui: user-management flows, App state, server config rendering

**Files:**
- Delete `crates/dux-tui/src/app/auth_users.rs`; remove `mod auth_users;`.
- `crates/dux-tui/src/app/mod.rs` — delete the `pending_auth_ops` field (currently ~212 to ~213) and its initializers (currently ~1775; plus the `..` shorthand sites), the three `PromptState` variants (`ServerAddUserName`/`ServerAddUserPassword`/`ServerRemoveUser`, currently ~961 to ~977), and the `server-add-user`/`server-remove-user` command handlers (currently ~2377 to ~2382).
- `crates/dux-tui/src/app/render.rs`, `input.rs`, `workers.rs` — delete the render/input/worker handling for the three prompts and `apply_auth_users_outcome` / the `EventReaction::AuthUsersOutcome` handling (currently `workers.rs` ~613) and the `pending_auth_ops` drain.
- `crates/dux-tui/src/app/sessions.rs`, `app/test_support.rs`, `app/mod.rs` — delete the `pending_auth_users: None`/`pending_auth_ops: ...` initializers. Do NOT trust these line hints; **grep every init site** with `rg -n 'pending_auth_users|pending_auth_ops' crates/` and remove each (known sites: `sessions.rs` ~3114, ~3305; `test_support.rs` ~165, ~253; `app/mod.rs` ~1648 for `pending_auth_users` and ~1775 for `pending_auth_ops`; plus `dux-web/src/bootstrap.rs` and `dux-core/src/engine/test_support.rs` for the `Engine` literal). Every `Engine { .. }` and `App { .. }` struct literal must drop the removed field.
- `crates/dux-tui/src/keybindings.rs` — delete the `Action::ServerAddUser`/`ServerRemoveUser` keybinding entries (currently ~1054, ~1061).
- `crates/dux-tui/src/config.rs` — delete the `[auth]` rendering; update the `[server]` renderer to document `host` and `allowed_hosts` and drop `listen_addrs`/`bind`/`insecure_allow_remote`/`dangerously_listen_http`; update `migrate_server_bind` per D-MIGRATE (Step below); validate `host` per D-HOSTVALIDATE.
- `crates/dux-tui/src/cli.rs` — delete the user-count print (currently ~369).
- `crates/dux-tui/src/server_screen.rs` — remove `auth_enabled: bool` and `user_count: usize` from `ServerStatusScreen::new` and delete the login row (currently ~120, ~427).

### 3d. dux binary: CLI flags + flip path

**Files:**
- `crates/dux/src/main.rs` — reduce `SERVER_USAGE`, `ServerArgs`, `parse_server_args`, `into_overrides` to `{ bind, port, no_tailscale }` (reject a second `--bind` with a clear error; add the test). Delete `--disable-auth`, `--insecure-allow-remote`, `--listen`, the old `--bind` push semantics. In the serve path, call `resolve_server_plan(server, cli, tailscale_ip)` (3 args) and `dux_web::run_server(paths, plan, version)` (no `disable_auth`). In the TUI flip path (currently ~78 to ~94), delete the `dux_core::auth::auth_enabled`/`parse_users` calls and drop `auth_enabled`/`user_count` from the `ServerStatusScreen::new` call.

### New code (the resolver and helpers)

`ServerConfig` has a hand-written `impl Default for ServerConfig` (a struct literal naming every field), NOT a derive. Adding `host`/`allowed_hosts` to the struct without updating this impl is a compile error. Update it in the same edit:

```rust
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            tailscale_enabled: true,
            allowed_hosts: Vec::new(),
            color: "auto".to_string(),     // keep the crate's existing default value
            access_log: true,              // keep the crate's existing default value
            max_websocket_connections: crate::config::DEFAULT_MAX_WEBSOCKET_CONNECTIONS,
        }
    }
}
```

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerPlan {
    pub addrs: Vec<PlanAddr>,
}

#[derive(Clone, Debug, Default)]
pub struct ServerCliOverrides {
    /// `--bind <ADDR:PORT>`: bind this exact address, overriding config host+port.
    pub bind: Option<String>,
    /// `--port <PORT>`: override `[server] port` only. Ignored when `bind` is set.
    pub port: Option<u16>,
    /// `--no-tailscale`: do not bind the Tailscale leg this run.
    pub no_tailscale: bool,
}

pub fn resolve_server_plan(
    server: &ServerConfig,
    cli: &ServerCliOverrides,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Result<ServerPlan> {
    let bind: std::net::SocketAddr = match cli.bind.as_deref() {
        Some(raw) => raw.parse().map_err(|_| {
            anyhow!(
                "invalid --bind address \"{raw}\": expected IP:port, e.g. 0.0.0.0:8080 \
                 (hostnames are not resolved)"
            )
        })?,
        None => {
            let host: std::net::IpAddr = server.host.parse().map_err(|_| {
                anyhow!(
                    "invalid [server] host \"{}\": expected an IP address such as 127.0.0.1 \
                     or 0.0.0.0 (hostnames are not resolved). Set [server] host in config.toml \
                     or pass --bind IP:port.",
                    server.host
                )
            })?;
            std::net::SocketAddr::new(host, cli.port.unwrap_or(server.port))
        }
    };
    if bind.port() == 0 {
        bail!(
            "refusing to bind {bind}: port 0 means \"pick any free port\", so there would be no \
             stable address to open. Set [server] port (default 8080) or pass --port / --bind with \
             a non-zero port."
        );
    }
    let ts = if server.tailscale_enabled && !cli.no_tailscale {
        tailscale_ip
    } else {
        None
    };
    Ok(ServerPlan { addrs: plan_addrs(bind, ts) })
}

/// Primary address (REQUIRED) plus the Tailscale leg (BEST-EFFORT) when detected and
/// not already covered. A wildcard primary (0.0.0.0 / ::) already binds the Tailscale
/// interface, and an explicit bind to the Tailscale address is already in the list, so
/// both cases skip the extra leg.
pub(crate) fn plan_addrs(
    bind: std::net::SocketAddr,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Vec<PlanAddr> {
    let mut addrs = vec![PlanAddr::required(bind)];
    if let Some(ip) = tailscale_ip {
        let ts = std::net::SocketAddr::new(ip, bind.port());
        let subsumed = bind.ip().is_unspecified() || bind.ip() == ip;
        if !subsumed && !addrs.iter().any(|p| p.addr() == ts) {
            addrs.push(PlanAddr::best_effort(ts));
        }
    }
    addrs
}

/// LOCAL MODE bind addresses for the TUI palette flip: loopback (REQUIRED) plus the
/// Tailscale leg. A thin wrapper over `plan_addrs` so the flip can never open a
/// non-loopback primary listener.
pub fn local_addrs(port: u16, tailscale_ip: Option<std::net::IpAddr>) -> Vec<PlanAddr> {
    plan_addrs(std::net::SocketAddr::from(([127, 0, 0, 1], port)), tailscale_ip)
}
```

### Tests for Task 3 (use REAL helpers; these existed-helper names were verified)

Resolver unit tests in `config.rs` (replace the old ACME/full-web tests; keep `local_addrs_tests`):

```rust
#[cfg(test)]
mod resolve_plan_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    fn ts() -> IpAddr { IpAddr::V4(Ipv4Addr::new(100, 100, 0, 1)) }
    fn cli() -> ServerCliOverrides { ServerCliOverrides::default() }

    #[test] fn default_loopback_only_without_tailscale() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), None).unwrap();
        assert_eq!(p.addrs, vec![PlanAddr::required("127.0.0.1:8080".parse().unwrap())]);
    }
    #[test] fn default_adds_best_effort_tailscale_leg() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), Some(ts())).unwrap();
        assert_eq!(p.addrs.len(), 2);
        assert!(!p.addrs[1].is_required());
    }
    #[test] fn no_tailscale_suppresses_leg() {
        let c = ServerCliOverrides { no_tailscale: true, ..cli() };
        assert_eq!(resolve_server_plan(&ServerConfig::default(), &c, Some(ts())).unwrap().addrs.len(), 1);
    }
    #[test] fn bind_wildcard_overrides_and_subsumes_tailscale() {
        let c = ServerCliOverrides { bind: Some("0.0.0.0:9000".into()), ..cli() };
        let p = resolve_server_plan(&ServerConfig::default(), &c, Some(ts())).unwrap();
        assert_eq!(p.addrs, vec![PlanAddr::required("0.0.0.0:9000".parse().unwrap())]);
    }
    #[test] fn port_flag_overrides_only_port() {
        let c = ServerCliOverrides { port: Some(7000), ..cli() };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(p.addrs, vec![PlanAddr::required("127.0.0.1:7000".parse().unwrap())]);
    }
    #[test] fn bind_beats_port() {
        let c = ServerCliOverrides { bind: Some("127.0.0.1:1234".into()), port: Some(7000), ..cli() };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(p.addrs, vec![PlanAddr::required("127.0.0.1:1234".parse().unwrap())]);
    }
    #[test] fn port_zero_refused() {
        let mut c = ServerConfig::default(); c.port = 0;
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
    #[test] fn invalid_bind_refused() {
        let c = ServerCliOverrides { bind: Some("nope".into()), ..cli() };
        assert!(resolve_server_plan(&ServerConfig::default(), &c, None).is_err());
    }
    #[test] fn invalid_host_refused() {
        let mut c = ServerConfig::default(); c.host = "example.com".into();
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
}
```

Config round-trip test in `config_write.rs` (use the REAL `write_config_plain` + `toml::from_str`, mirroring `write_config_plain_round_trips_server_section` currently ~811):

```rust
#[test]
fn write_config_plain_round_trips_host_and_allowed_hosts() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("c.toml");
    let mut cfg = Config::default();
    cfg.server.host = "0.0.0.0".into();
    cfg.server.port = 9000;
    cfg.server.allowed_hosts = vec!["box.tailnet.ts.net".into()];
    write_config_plain(&path, &cfg).unwrap();
    let parsed: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(parsed.server.host, "0.0.0.0");
    assert_eq!(parsed.server.port, 9000);
    assert_eq!(parsed.server.allowed_hosts, vec!["box.tailnet.ts.net".to_string()]);
}
```

Config renderer test in `dux-tui/config.rs` (use the REAL `render_default_config()` which takes no args, mirroring the test at ~1253):

```rust
#[test]
fn rendered_server_section_is_local_only() {
    let toml = render_default_config();
    assert!(toml.contains("host = \"127.0.0.1\""));
    assert!(toml.contains("allowed_hosts"));
    assert!(!toml.contains("[server.acme]"));
    assert!(!toml.contains("listen_addrs"));
    assert!(!toml.contains("[auth]"));
}
```

CLI parse tests in `main.rs` (reduced surface, plus the double-`--bind` rejection):

```rust
#[test] fn bind_parses_once() {
    assert_eq!(ok(&["--bind", "0.0.0.0:8888"]).bind.as_deref(), Some("0.0.0.0:8888"));
}
#[test] fn second_bind_is_rejected() { assert!(err(&["--bind", "a:1", "--bind", "b:2"]).contains("once")); }
#[test] fn removed_flags_unknown() {
    for f in ["--listen","--disable-auth","--insecure-allow-remote","--acme-domain",
              "--no-acme","--dangerously-listen-http"] {
        assert!(err(&[f]).contains("unknown argument") || err(&[f, "x"]).contains("unknown argument"));
    }
}
```

D-HOSTVALIDATE: add an `IpAddr::from_str` validation of `server.host` in `ensure_config` in `crates/dux-tui/src/config.rs` (it returns `Result<Config>` and runs before the TUI starts, so it can reject; `load_config` in dux-core returns a bare `Config` and cannot). Return a clear error on a non-IP host; add a test that a non-IP host is rejected there. `resolve_server_plan` keeps its own `?` validation as the server-start backstop.

D-MIGRATE: rewrite `migrate_server_bind` (currently ~249 in `dux-tui/config.rs`) so a non-loopback `bind` writes its IP into `host` and port into `port` (with a `logger::warn`) instead of into the removed `listen_addrs`; a loopback `bind` is dropped silently. Add a test that an old `bind = "0.0.0.0:9000"` migrates to `host = "0.0.0.0"`, `port = 9000`.

- [ ] **Step 1: Delete frontend-independent backend auth in dependency order:** start at the leaves (`dux-tui` user flows, `keybindings`, `main.rs` flip path, CLI flags, palette/action match arms, `wire.rs` arms), then the dux-web surface (server.rs gate/routes/tests, engine_actor, lib.rs, test_support), then the dux-core engine machinery (events.rs/mod.rs/in_flight.rs/worker.rs), then `auth.rs` files and `AuthConfig`, then collapse `ServerPlan`/`resolve_server_plan`/`ServerConfig` and add `host`/`allowed_hosts`, then `config_write` and the renderer + migration + load-validation.
- [ ] **Step 2: Write the resolver/CLI/config tests above; run them.**
- [ ] **Step 3: Remove `bcrypt` from `crates/dux-core/Cargo.toml` AND the workspace root `Cargo.toml` (currently ~17).**
- [ ] **Step 4: Full gate** (fmt, clippy -D warnings, build --workspace, test --workspace). Run the stale-symbol sweep (Task 6 Step 2) and confirm zero auth/acme/exposure survivors.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Remove the web auth and users subsystem and serve a single local bind"
```

---

## Task 4: DNS-rebinding guard and REST same-origin protection

**Files:**
- `crates/dux-web/src/host_guard.rs` — generalize the relocated `DomainAllowlist` into a `HostAllowlist` for the local flow; add a tower layer used by the router.
- `crates/dux-web/src/server.rs` — apply the host guard to the whole router; add a server-side same-origin check to REST mutation routes (POST/PATCH/PUT/DELETE) mirroring the existing `same_origin_allowed` (full `host:port` authority comparison). Keep the three WS `same_origin_allowed` checks.
- `crates/dux-web/src/lib.rs` — thread the bound IP literals and `server.allowed_hosts` into the guard at router build.

**Interfaces:**
- `pub struct HostAllowlist` with `HostAllowlist::new(bound_ips: &[IpAddr], configured: &[String]) -> Self` and `fn allows_host(&self, host_header: &str) -> bool`. A request whose `Host` is not allowed gets `403`. **Strip any `:port` suffix (IPv6-bracket-aware) and lowercase the host before every comparison** by reusing the relocated `normalize_host_for_match`/`strip_host_port` helpers, so a configured `box.tailnet.ts.net` matches `Host: box.tailnet.ts.net:8080`. Allow rules (NO wildcard, per D-WILDCARD):
  1. The host is a loopback literal (`localhost`, `127.0.0.1`, `::1`/`[::1]`).
  2. **If any `bound_ips` entry is unspecified (`0.0.0.0` or `::`), accept any host that parses as an `IpAddr`.** A `0.0.0.0` bind is reachable at every local IP (e.g. `192.168.1.5`), so pinning to the literal `0.0.0.0` would 403 all real LAN clients. This is safe: a DNS-rebinding attacker cannot make a browser send an IP-literal `Host` for a hostname they control, and binding `0.0.0.0` is already an intentional LAN exposure. This mirrors the `is_unspecified()` check in `plan_addrs`.
  3. The host parses as an `IpAddr` that is in `bound_ips` (so the Tailscale `100.x` literal works out of the box).
  4. The host case-insensitively equals a (port-stripped) entry in `configured`.

  Note: a tailnet **MagicDNS** name (`box.tailnet.ts.net`) is NOT an IP literal, so out of the box it is only reachable via the `100.x` IP unless the user adds the name to `allowed_hosts`. The `allowed_hosts` config comment must say so (see Task 3a). Auto-detecting the MagicDNS name is punted to L1.

- [ ] **Step 1: Tests** for `allows_host` (loopback always; bound-IP literal allowed; configured hostname case-insensitive; everything else rejected; no `"*"` behavior). Run; they fail.
- [ ] **Step 2: Implement `HostAllowlist` + the layer**; drop the ACME-challenge exemption the old allowlist had. Apply it in `server.rs`.
- [ ] **Step 3: REST same-origin.** Add a check on mutation routes (POST/PATCH/PUT/DELETE): if an `Origin` header is present and its `host:port` authority does not match the `Host` authority (or an allowed host), return 403. **When `Origin` is present but its authority cannot be parsed (notably the literal value `null`, which browsers send from sandboxed iframes and `data:` documents), treat it as a cross-origin mismatch and return 403 — do NOT fall through to the no-Origin-present allow path.** A missing `Origin` (non-browser clients like curl) is allowed. Reuse/lift the existing `same_origin_allowed` authority comparison so REST and WS share one implementation (DRY).
- [ ] **Step 4: Serve-level tests:** a bad `Host` gets 403; `localhost` gets 200; a cross-origin POST to a mutation route gets 403; **a POST with `Origin: null` and a valid `Host` gets 403 regardless of bind address**; a POST with no `Origin` is allowed; cross-origin WS upgrades to all three WS endpoints get 403 (not just `/ws/events`).
- [ ] **Step 5: Full gate.**
- [ ] **Step 6: Commit**

```bash
git add -A crates/dux-web
git commit -m "Re-home DNS-rebinding protection and add REST same-origin checks"
```

---

## Task 5: Startup safety note

**Files:**
- `crates/dux-web/src/lib.rs` (plain-HTTP startup banner) and `crates/dux-web/src/console.rs` if a console helper is used.
- `crates/dux-tui/src/server_screen.rs` — the flip status screen banner matches (login row already removed in Task 3; add the same note).

**Interface:** a pure `safety_note(addrs: &[PlanAddr]) -> Option<String>` (ONE argument; a Tailscale leg is the `best_effort` non-loopback addr, identifiable from the list). The required (primary) address and the optional best-effort Tailscale leg are INDEPENDENT, so classify on both, highest-severity-wins (a `--bind 192.168.1.5:8080` with Tailscale active yields BOTH a non-loopback primary AND a Tailscale leg). Note text (no em-dashes):
- Primary is non-loopback (a custom host or `0.0.0.0`): `"Reachable on your network with NO login. Anyone who can reach this address controls your agents and worktrees. Put it behind Tailscale or a trusted reverse proxy."` If a Tailscale leg is ALSO bound, append: `" (The Tailscale address is bound too.)"` The LAN warning wins because a LAN IP is reachable by anyone on the LAN, not just the tailnet.
- Primary is loopback but a Tailscale leg is bound: `"Reachable by other devices on your tailnet (no login). Disable with tailscale_enabled = false under [server]."`
- Loopback only: `None` (calm one-liner elsewhere, no warning).

- [ ] **Step 1: Tests** for `safety_note(&addrs)`: loopback-only -> None; loopback + tailscale leg -> contains "tailnet"; `0.0.0.0` primary -> contains "NO login"; **`192.168.1.5` primary + tailscale leg -> contains "NO login" (the overlap case)**. Run; fail.
- [ ] **Step 2: Implement** and wire into the `dux server` banner and the TUI flip screen (TUI uses `theme.rs` warning tone).
- [ ] **Step 3: Full gate** (`cargo test --workspace`).
- [ ] **Step 4: Commit**

```bash
git add -A crates/dux-web crates/dux-tui
git commit -m "Add a trusted-local startup safety note"
```

---

## Task 6: Documentation, website, and stale-symbol sweep

**Files:** `CLAUDE.md`, `docs/server-mode-summary.md`, `website/docs/*` (server-mode/config pages), `README.md`.

- [ ] **Step 1: Grep and rewrite** every stale reference:

```bash
rg -n -i "acme|let'?s encrypt|--listen|--disable-auth|insecure-allow-remote|dangerously-listen-http|server-add-user|server-remove-user|\[auth\]|listen_addrs|http-01|https" README.md CLAUDE.md docs/ website/
```

Update CLAUDE.md (keep the single-tenant + no-per-user-isolation tenet; drop the public-bind gate, `--disable-auth`, ACME, `[auth]`, and the `acme` keyed-status example). Document `host`, `port`, `tailscale_enabled`, `allowed_hosts`, and "front with Tailscale or a proxy". Match the website's playful tone; no keybinding enumeration; no values that drift.

- [ ] **Step 2: Whole-tree stale-symbol sweep** (must return nothing but intended survivors):

```bash
rg -n -i "rustls_acme|axum_server|AcmeSettings|ServerPlan::Acme|resolve_acme_cache_dir|AuthConfig|SweepableMemoryStore|tower_sessions|tower-sessions|bcrypt|RateLimiter|secure_cookie|RouterParams::tls|insecure_allow_remote|dangerously_listen_http|listen_addrs|ServerAddUser|ServerRemoveUser|AuthUsersPersisted|AuthUsersOutcome|AuthUserFinalOutcome|pending_auth_users|pending_auth_ops|host_only|disable_auth|render_server_acme_config|login_row" crates/ Cargo.toml
```

- [ ] **Step 3: Build the site if it has a build; otherwise verify Markdown frontmatter.** Final full gate (fmt, clippy, build --workspace, test --workspace; `npm run build && npm test`).
- [ ] **Step 4: Manual smoke (ASK THE USER per CLAUDE.md; do not auto-run):** `cargo run -- server` (loopback + Tailscale leg + safety note); `--bind 0.0.0.0:8888` ("NO login" note); `--no-tailscale` (loopback only); the TUI flip still serves with the matching banner and no login row.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Document the local and Tailscale plain HTTP server with no login"
```

---

## Punted / low-priority (recorded, not done in the main path)

- **L1 (MagicDNS auto-allow):** auto-detect the machine's `*.ts.net` name (extend Tailscale detection to read `Self.DNSName` from `tailscale status --json`) and add it to the allowlist so tailnet access works with zero `allowed_hosts` config. Until then, tailnet users reaching dux via MagicDNS must add their name to `allowed_hosts`. Worth a follow-up; not required for correctness.
- **L2 (`--port` + `--bind` together):** chosen behavior is "`--bind` wins, `--port` ignored," documented in `SERVER_USAGE`. Could be a hard error instead; cheap to change if preferred.
- **Applied inline (were low findings):** dropped the non-idiomatic compile-guard struct-literal test (keep the defaults test + round-trip test); `plan_addrs` is `pub(crate)` not `pub`; reject a repeated `--bind`; REST/WS origin checks compare full `host:port` authority (not host alone) so a different port on the same IP stays cross-origin.

## Self-Review (against the spec and the review findings)

- Astro-style local default + Tailscale leg unless disabled -> Task 3 resolver + `host` default loopback.
- `tls.rs` removed, not collapsed -> Task 2 (file deleted; allowlist relocated first; Task 6 sweep proves no TLS symbol survives).
- DNS-rebinding kept + integrated, plus the CSRF gap from removing auth -> Task 4 (host allowlist re-homed; REST + 3 WS same-origin).
- Safety note naming the Tailscale opt-out -> Task 5.
- Host+port via config and flags -> Task 3 (`host`, `--bind`, `--port`).
- Remove auth entirely incl. the engine reload barrier -> Task 3 (3a engine machinery, 3b dux-web, 3c dux-tui, 3d binary) + Task 1 (frontend).
- Remove `--disable-auth` and `run_server(disable_auth)` -> Task 3d / 3b.
- Migration safety for old configs -> D-MIGRATE in Task 3; `host` validated at load (D-HOSTVALIDATE).
- No `"*"` wildcard -> D-WILDCARD.
- Workspace-level green gate replaces per-crate gate -> stated up front; each task ends with `build --workspace && test --workspace`.
