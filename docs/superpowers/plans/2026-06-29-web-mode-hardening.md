# Web-Mode Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve six deferred web-server hardening items on the `server-mode` branch (reconnect convergence, per-class connection caps, cheaper change-detection, proactive PTY-viewer cleanup, REST route nesting, detached-HEAD tolerance) plus a per-PTY active-owner model with read-only secondary views.

**Architecture:** A Rust axum server (`crates/dux-web`) wraps a single-threaded engine actor (`crates/dux-core`) and serves a React/Vite SPA (`crates/dux-web/web`). All reads/actions are REST (`/api/v1/*`); a single `/ws/events` socket pushes resource-change/status signals; per-PTY binary sockets stream terminals. State is in-memory except SQLite session/changes persistence. Single-tenant, trusted-access by design.

**Tech Stack:** Rust (axum, tokio, rusqlite, portable-pty, alacritty_terminal), TypeScript/React/Vite/Tailwind v4 (shadcn/base-ui), TOML config via `toml_edit`.

**Source spec:** `docs/superpowers/specs/2026-06-29-web-hardening-plan.md` (reviewed by a 14-charter adversarial panel; this plan is its executable form).

## Global Constraints

- **Targets are macOS + Linux only.** No `#[cfg(windows)]` / `cfg!(windows)` branches.
- **Config is the documentation.** Every new config key gets an inline comment explaining it (purpose, default, caveats) in the canonical renderer.
- **Derived state lives in SQLite/memory, portable intent in config.** Do not write runtime-derived state into `config.toml`.
- **Status line: every `Busy` is followed by a final success/error/clear**, correlated by key; web renders keyed statuses as toasts.
- **Git output must be config-immune:** plumbing over porcelain; `--porcelain=v1 -z` / `--numstat -z` when parsing; `-c color.x=false` otherwise. Rely on exit status for imperative commands.
- **Never byte-slice user-visible strings** — use `.chars().count()` / `.chars().take(n)`.
- **Web UI is dark-only**, style via shadcn/base-ui CSS tokens, never hardcoded colors; row actions collapse into a `⋯` `DropdownMenu`; destructive actions confirm via a dialog; hover hints via `SimpleTooltip`; touch targets ≥44px (`max-md:min-h-11`).
- **No em-dashes** in code or prose (user preference).
- **Verification (CI gates), run before every commit:**
  - Rust: `cargo fmt` ; `cargo clippy --all-targets --all-features -- -D warnings` ; `cargo test`
  - Web (from `crates/dux-web/web`): `npx tsc -b` ; `npx vitest run`
- **Commit messages are plain sentences**, no conventional-commit prefixes, no structured trailers.

## Decisions locked in (from design discussion)

- **Three connection caps**, deliberately separate: events / agent-PTY / terminal-PTY, defaults **32 / 32 / 64** (totals 128, same as the old single default). `=0` permanently blocks that class until restart.
- **Item 1 catch-up is fine-topic only** (`session:<id>:changes`); coarse topics stay on the existing `onOpen` refetch.
- **Per-PTY active-owner model:** most-recent **foreground** attach owns and drives size and **input**; it sends its size on attach/takeover so the PTY snaps to fit; the displaced device shows a clean **placeholder** ("Active on another device — Take over") and is **read-only**; a backgrounded tab silently reconnecting does **not** steal ownership.
- **Notification-tag validation:** a REST mutation's client-supplied connection id is validated against a live-connection registry; an unknown/forged id falls back to broadcasting status to **all** clients.

## File Structure (what changes and why)

**Rust — `crates/dux-core/src/`**
- `git.rs` — add `current_branch_opt` (detached-tolerant); make `switch_branch_if_needed` detached-tolerant.
- `project_browser.rs` — `leading_branch_for_project` signature → `Option<&str>`; `load_projects` + branch-status/checkout-default jobs use `current_branch_opt`.
- `agent_job.rs` — base-branch fallback routes through `leading_branch_for_project`.
- `config.rs` — remove `max_websocket_connections`; add three cap fields.
- `config_write.rs` — strip old key on every save; render three new commented keys.
- `wire.rs` — web add-project paths use `current_branch_opt`.

**Rust — `crates/dux-web/src/`**
- `server.rs` — three semaphores; events-handler permit via helper; PTY active-owner read-only enforcement + size-on-attach already partly present (`PtySizeOwners`); a `ConnectionRegistry`; liveness ping; access-log comment fix; nested git/file route registration.
- `engine_actor.rs` — `server_rebind_settings_changed` three-field compare; spine-check gating (mutation version + streaming counter + backstop + `cfg(test)` call counter).
- `git_routes.rs`, `file_routes.rs` — nested under `/api/v1/sessions/:id/...`, `Path<String>` id, `id_within_bound` guard.
- `rest_common.rs` — connection-id validation helper.
- `changes.rs` — reuse `peek_rev` for catch-up (no change beyond exposing if needed).
- `project_reads.rs` — inspect tolerates detached HEAD.
- `event_bus.rs` — no change expected (interest ref-count stays).

**Rust — `crates/dux-tui/src/`**
- `config.rs` — canonical renderer + tests for the three new keys.
- `cli.rs` — `config diff` for the three new keys.
- `app/sessions.rs`, `app/workers.rs` — TUI add-project / create-agent pre-check use `current_branch_opt`.
- `lib.rs`, `tests/auth_gate.rs` — field-reference updates.

**Web — `crates/dux-web/web/src/`**
- `lib/git.ts`, `lib/fileApi.ts`, `lib/markdown.ts` — nested URLs, drop `session_id` from bodies/query.
- `lib/gitFileApi.test.ts`, `lib/markdown.test.ts` — updated URL/body assertions.
- PTY view component(s) + `lib/store.ts` / `lib/eventsSocket.ts` — send size on attach; render placeholder + read-only for displaced views; foreground-aware ownership.

---

## Task ordering and dependencies

Tasks are independent and land as separate commits, except: **Task 6 (active-owner)** builds on the existing `PtySizeOwners`; **Task 7 (liveness + tag validation)** introduces the `ConnectionRegistry` reused by tag validation. Recommended order: **1 → 2 → 3 → 4 → 5 → 6 → 7 → 8**.

---

## Task 1: Detached-HEAD-tolerant git helper

**Files:**
- Modify: `crates/dux-core/src/git.rs` (after `current_branch`, ~line 78)
- Test: `crates/dux-core/src/git.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn current_branch_opt(repo_path: &Path) -> anyhow::Result<Option<String>>` — `Ok(Some(name))` on a normal branch (trimmed); `Ok(None)` **iff** `git symbolic-ref` exits with code `1` (detached HEAD); `Err` on any other non-zero exit (128 = not a repo, etc.) or spawn failure.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn current_branch_opt_returns_branch_on_normal_head() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(&tmp.path().to_path_buf()); // existing helper: init -b main + commit
    assert_eq!(current_branch_opt(tmp.path()).unwrap(), Some("main".to_string()));
}

#[test]
fn current_branch_opt_returns_none_on_detached_head() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().to_path_buf();
    init_repo(&p);
    // create a second commit, then detach onto the first
    std::fs::write(p.join("f"), b"x").unwrap();
    run_git(&p, &["add", "."]);
    run_git(&p, &["commit", "-m", "second"]);
    run_git(&p, &["checkout", "--detach", "HEAD~1"]);
    assert_eq!(current_branch_opt(&p).unwrap(), None);
}

#[test]
fn current_branch_opt_errors_on_non_repo() {
    let tmp = tempfile::tempdir().unwrap(); // not a git repo
    assert!(current_branch_opt(tmp.path()).is_err());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dux-core current_branch_opt`
Expected: FAIL — `cannot find function current_branch_opt`.

- [ ] **Step 3: Implement `current_branch_opt`**

```rust
/// Like [`current_branch`], but tolerates a detached HEAD: returns `Ok(None)`
/// when HEAD is not a symbolic ref (git `symbolic-ref` exit code 1, with
/// `--quiet` suppressing the message), and `Err` for any real failure
/// (exit 128 = not a repo, git missing, etc.). Used by inspection/preview
/// call sites that must not treat a detached HEAD as a hard error.
pub fn current_branch_opt(repo_path: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo_path.to_string_lossy().as_ref(),
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to inspect {}", repo_path.display()))?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    // Exit code 1 = "ref is not a symbolic ref" (detached HEAD). Anything else
    // (128 = not a repo / fatal) is a real error. `--quiet` silenced stderr for
    // the detached case only.
    if output.status.code() == Some(1) {
        return Ok(None);
    }
    Err(anyhow!(
        "git symbolic-ref failed for {}: {}",
        repo_path.display(),
        String::from_utf8_lossy(&output.stderr)
    ))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dux-core current_branch_opt`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/dux-core/src/git.rs
git commit -m "Add detached-HEAD-tolerant current_branch_opt git helper"
```

---

## Task 2: Detached-tolerant leading-branch derivation and switch

**Files:**
- Modify: `crates/dux-core/src/project_browser.rs` (`leading_branch_for_project` ~line 71; `load_projects` ~line 105; branch-status job ~line 197; checkout-default inspection ~line 224)
- Modify: `crates/dux-core/src/git.rs` (`switch_branch_if_needed` ~line 231)
- Test: both files' test modules

**Interfaces:**
- Changes: `pub fn leading_branch_for_project(path: &Path, current_branch: Option<&str>) -> String` — prefers the remote default branch; falls back to `current_branch` when `Some`; when both are unavailable returns the existing heuristic default (keep current behavior for the `Some` path; only the signature and the `None` handling are new).
- Consumes: `current_branch_opt` (Task 1).

- [ ] **Step 1: Write the failing test (switch tolerates detached)**

```rust
#[test]
fn switch_branch_if_needed_switches_from_detached_head() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().to_path_buf();
    init_repo(&p);                 // on main
    run_git(&p, &["checkout", "--detach", "HEAD"]);
    // Must not error on detached HEAD; must end up on main.
    switch_branch_if_needed(&p, "main").unwrap();
    assert_eq!(current_branch_opt(&p).unwrap(), Some("main".to_string()));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p dux-core switch_branch_if_needed_switches_from_detached_head`
Expected: FAIL — current `switch_branch_if_needed` calls `current_branch(...)?` which errors on detached HEAD.

- [ ] **Step 3: Make `switch_branch_if_needed` detached-tolerant**

```rust
pub fn switch_branch_if_needed(repo_path: &Path, branch: &str) -> Result<()> {
    // On a detached HEAD there is no current branch to compare against, so we
    // simply switch. Only skip the switch when already on the target branch.
    let current = current_branch_opt(repo_path)?;
    if current.as_deref() != Some(branch) {
        switch_branch(repo_path, branch)?;
    }
    Ok(())
}
```

- [ ] **Step 4: Change `leading_branch_for_project` signature and `None` handling**

```rust
pub fn leading_branch_for_project(path: &Path, current_branch: Option<&str>) -> String {
    match remote_default_branch(path) {
        Some(def) => def,
        // No remote default: fall back to the current branch if we have one,
        // else the heuristic default (keep the pre-existing default string).
        None => current_branch
            .map(|s| s.to_string())
            .unwrap_or_else(|| DEFAULT_LEADING_BRANCH.to_string()), // existing default literal
    }
}
```

Update the three callers in `project_browser.rs`:
- `load_projects` (~line 105-113): replace `git::current_branch(&path).unwrap_or_else(|_| "main".to_string())` with
  ```rust
  let current_branch = if missing { String::new() }
      else { git::current_branch_opt(&path).ok().flatten().unwrap_or_default() };
  ```
  and call `leading_branch_for_project(&path, (!current_branch.is_empty()).then_some(current_branch.as_str()))`.
- Branch-status job (~line 197) and checkout-default inspection (~line 224): replace `git::current_branch(&repo_path)` with `git::current_branch_opt(&repo_path)`, treating `Ok(None)` as "no current branch" (empty) rather than an error.

- [ ] **Step 5: Run to verify the switch test passes**

Run: `cargo test -p dux-core switch_branch_if_needed_switches_from_detached_head`
Expected: PASS.

- [ ] **Step 6: Fix the compile errors at every `leading_branch_for_project` caller**

Run: `cargo build -p dux-core -p dux-tui -p dux-web`
Wrap each existing `&current_branch` arg as `Some(current_branch.as_str())` (TUI `app/sessions.rs:58`, `app/workers.rs:1393`, and any test fixtures). Expected after fixes: clean build.

- [ ] **Step 7: Run the full core suite + commit**

```bash
cargo test -p dux-core
git add crates/dux-core/src/git.rs crates/dux-core/src/project_browser.rs crates/dux-core/src/agent_job.rs
git commit -m "Make leading-branch derivation and branch switching tolerate a detached HEAD"
```

---

## Task 3: Detached-HEAD tolerance across all inspection call sites (web + TUI)

**Files:**
- Modify: `crates/dux-web/src/project_reads.rs` (`inspect_path` ~line 170-195)
- Modify: `crates/dux-core/src/wire.rs` (`add_project_checkout_default` ~line 1238; `AddProject` handler ~line 2080)
- Modify: `crates/dux-core/src/agent_job.rs` (base fallback ~lines 60-63, 95-98)
- Modify: `crates/dux-tui/src/app/sessions.rs` (~line 57), `crates/dux-tui/src/app/workers.rs` (~line 1389)
- Test: `crates/dux-web/src/project_reads.rs` tests; `crates/dux-core/src/agent_job.rs`/`wire.rs` tests

**Interfaces:**
- Consumes: `current_branch_opt`, `leading_branch_for_project(Option<&str>)` (Tasks 1-2).

- [ ] **Step 1: Write failing web inspect tests**

```rust
#[tokio::test]
async fn inspect_detached_head_reports_null_branch_200() {
    // build a detached-HEAD repo in a tempdir, then GET the inspect route
    // assert: HTTP 200 and body { "current_branch": null, ... }
}

#[tokio::test]
async fn inspect_non_repo_still_400() {
    // existing inspect_non_repo_reports_error must keep returning 400
}
```

- [ ] **Step 2: Run to verify the detached test fails (and non-repo still passes)**

Run: `cargo test -p dux-web inspect_detached_head inspect_non_repo`
Expected: detached test FAILS (currently 400), non-repo PASSES.

- [ ] **Step 3: Make `inspect_path` use `current_branch_opt`**

In `project_reads.rs` the blocking closure becomes:
```rust
let branch = dux_core::git::current_branch_opt(repo).map_err(|e| format!("{e:#}"))?;
let warning = match branch.as_deref() {
    Some(b) => dux_core::git::branch_warning_kind(repo, b).map(/* existing mapping */),
    None => None, // detached: no "not on default branch" warning
};
Ok::<_, String>((branch, warning))
```
and the reply uses `current_branch: branch` (already `Option<String>`). The non-repo path still returns `Err` → 400.

- [ ] **Step 4: Make the web add-project paths detached-tolerant**

In `wire.rs:1238` and `wire.rs:2080`, replace `crate::git::current_branch(&validated)?` with:
```rust
let branch = crate::git::current_branch_opt(&validated)?; // Option<String>
let leading = crate::git::leading_branch_for_project(&validated, branch.as_deref());
```
and store `current_branch` as `branch.unwrap_or_default()` (empty when detached).

- [ ] **Step 5: Route the create-agent base fallback through `leading_branch_for_project`**

In `agent_job.rs` (~60-63 and ~95-98) replace
`project.leading_branch.clone().unwrap_or_else(|| project.current_branch.clone())`
with
```rust
let leading_branch = project.leading_branch.clone().unwrap_or_else(|| {
    let cur = (!project.current_branch.is_empty()).then_some(project.current_branch.as_str());
    crate::git::leading_branch_for_project(&repo_path, cur)
});
```
If `local_branch_exists` is still false after this, the existing `CreateAgentFailed` path already produces a clear message — keep it.

- [ ] **Step 6: Make the TUI add-project + pre-check use `current_branch_opt`**

`app/sessions.rs:57`: `let branch = git::current_branch_opt(&path)?.unwrap_or_default();`
`app/workers.rs:1389`: change the inspection to use `current_branch_opt` and proceed with the stored/derived leading branch when `None`.

- [ ] **Step 7: Write a web create-agent detached test**

```rust
#[tokio::test]
async fn create_agent_detached_with_origin_head_uses_default_branch() {
    // project with leading_branch=None, repo detached but origin/HEAD -> main
    // assert: worktree job targets the default branch, not a faked "main" literal
}
```

- [ ] **Step 8: Run all affected tests + commit**

```bash
cargo test -p dux-core -p dux-web -p dux-tui
git add crates/dux-web/src/project_reads.rs crates/dux-core/src/wire.rs crates/dux-core/src/agent_job.rs crates/dux-tui/src/app/sessions.rs crates/dux-tui/src/app/workers.rs
git commit -m "Tolerate detached HEAD across project inspection, add-project, and create-agent paths"
```

---

## Task 4: Catch-up on (re)subscribe to close the changes-pane reconnect gap

**Files:**
- Modify: `crates/dux-web/src/server.rs` (`apply_events_frame` ~1489-1557; its caller in `handle_events_socket` ~line 1448; mirror the lag-recovery block ~1322-1376)
- Test: `crates/dux-web/src/server.rs` tests; `crates/dux-web/web/src/lib/eventsSocket` or store test for revless handling

**Interfaces:**
- Changes: `apply_events_frame(...) -> Vec<String>` returning the **newly-inserted fine topics** (the `session:<id>:changes` topics added by this frame). Caller emits a catch-up `session.changes` per new topic using the in-scope `sink` and `changes.peek_rev(sid)`.
- Consumes: `changes.peek_rev(session_id) -> Option<u64>` (changes.rs:403); `send_event(&sink, &frame)`.

- [ ] **Step 1: Write the failing gap-scenario test**

```rust
#[tokio::test]
async fn subscribe_emits_catchup_with_current_rev() {
    // seed a ChangesService so peek_rev(s1) == Some(N) for a real session
    // drive apply_events_frame with { subscribe: ["session:s1:changes"] }
    // assert: a session.changes event for s1 is sent on the sink carrying rev N
}

#[tokio::test]
async fn subscribe_cold_cache_emits_revless_catchup() {
    // peek_rev(s1) == None
    // assert: a session.changes event for s1 is sent with rev omitted (null)
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-web subscribe_emits_catchup subscribe_cold_cache`
Expected: FAIL — no catch-up is sent today.

- [ ] **Step 3: Refactor `apply_events_frame` to return newly-subscribed fine topics**

Change the body so that when inserting a topic into `subscribed`, if it is a `session:<id>:changes` topic and was newly inserted, collect it; return `Vec<String>` of those topic strings. (Coarse topics are not collected.)

- [ ] **Step 4: Emit catch-up at the caller**

After the `apply_events_frame(...)` call (~line 1448), for each returned fine topic, parse the session id (reuse the existing `changes_topic`/parse helper) and:
```rust
let frame = WireEvent { event: "session.changes".into(), id: Some(sid.clone()), rev: changes.peek_rev(&sid) };
send_event(&sink, &frame).await;
```
This mirrors the lag-recovery block at ~1322-1376 (same `sink`/`changes` in scope). `rev: None` serializes to an absent field (`skip_serializing_if = "Option::is_none"`), which the client treats as force-refetch.

- [ ] **Step 5: Run to verify the Rust tests pass**

Run: `cargo test -p dux-web subscribe_emits_catchup subscribe_cold_cache`
Expected: PASS.

- [ ] **Step 6: Add/verify the frontend revless-handling test**

In `crates/dux-web/web/src/lib/`, add a test asserting that handling a `session.changes` event for the selected session with `rev === undefined` calls `loadChanges` (the existing store logic at store.ts:519 already does this — assert it so a regression is caught).

Run (from `crates/dux-web/web`): `npx vitest run`
Expected: PASS.

- [ ] **Step 7: Gates + commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test -p dux-web
( cd crates/dux-web/web && npx tsc -b && npx vitest run )
git add crates/dux-web/src/server.rs crates/dux-web/web/src/lib/
git commit -m "Send a per-session catch-up on subscribe so the changes pane converges after a reconnect"
```

---

## Task 5: Proactive PTY-viewer cleanup (RAII unsubscribe)

**Files:**
- Modify: `crates/dux-core/src/pty.rs` (subscriber store ~line 427; `subscribe`/`subscribe_with_repaint` ~663/678; reader loop drain ~577)
- Test: `crates/dux-core/src/pty.rs` tests (existing tests spawn a real PTY, e.g. `/bin/cat`)

**Interfaces:**
- Changes: subscriber store becomes `Vec<(u64, Sender<Vec<u8>>)>` with a monotonic id counter on `PtyClient`.
- Produces: `subscribe(...) -> (PtyViewerGuard, Receiver<Vec<u8>>)` (and `subscribe_with_repaint` returns the guard alongside its existing tuple). Dropping the guard removes the subscriber promptly without waiting for PTY output.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dropping_the_guard_removes_subscriber_without_output() {
    let client = PtyClient::spawn_for_test("/bin/cat"); // existing test spawn helper
    let (guard, rx) = client.subscribe();
    drop(guard);
    // No PTY output is produced; the receiver must observe disconnection.
    assert!(matches!(rx.recv(), Err(_))); // all senders for this slot gone
}

#[test]
fn dropping_one_guard_keeps_the_other_subscriber() {
    let client = PtyClient::spawn_for_test("/bin/cat");
    let (g1, rx1) = client.subscribe();
    let (_g2, rx2) = client.subscribe();
    drop(g1);
    assert!(matches!(rx1.recv(), Err(_)));               // removed
    assert!(matches!(rx2.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty))); // still held
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-core dropping_the_guard dropping_one_guard`
Expected: FAIL — `subscribe` does not return a guard yet.

- [ ] **Step 3: Implement id-tagged subscribers + the guard**

- Change the store to `Arc<Mutex<Vec<(u64, Sender<Vec<u8>>)>>>` and add `next_sub_id: AtomicU64`.
- `subscribe` assigns an id, pushes `(id, tx)`, and returns a `PtyViewerGuard { id, subs: Arc<Mutex<...>> }` plus the `rx`.
- Implement `Drop for PtyViewerGuard` to remove its entry: `self.subs.lock().retain(|(id, _)| *id != self.id)`.
- Update the reader-loop drain (~577) to `subs.retain(|(_, tx)| tx.send(data.to_vec()).is_ok())` (still the reactive backstop).

**Async-safety note (review finding):** the guard's `Drop` takes a `std::sync::Mutex` and the guard lives in an async socket handler. At realistic counts the lock is held only briefly. If a mass-disconnect stall is a concern, prefer a removal queue (the guard pushes its id to a `Mutex<Vec<u64>>` that the reader loop drains before each fan-out) so the `Vec` mutation stays on the reader thread. Implement the simple direct `retain` first; add the queue only if a stress test shows contention.

- [ ] **Step 4: Update call sites in `server.rs`**

The PTY socket handlers (`handle_pty_socket`) must bind the guard for the socket's lifetime so it drops on disconnect. Update the `subscribe`/`subscribe_with_repaint` call sites to destructure the guard.

- [ ] **Step 5: Run tests + gates**

Run: `cargo test -p dux-core dropping_ ; cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/dux-core/src/pty.rs crates/dux-web/src/server.rs
git commit -m "Remove a PTY viewer immediately on disconnect via an RAII unsubscribe guard"
```

---

## Task 6: Per-PTY active-owner model (size + read-only secondary + placeholder)

Builds on the existing `PtySizeOwners` (most-recent attach owns sizing; non-owner resize ignored — server.rs:692-736, claimed at :923, gated at :968, released at :981).

**Files:**
- Modify: `crates/dux-web/src/server.rs` (PTY socket handlers: enforce input only from the owner; broadcast an ownership-change signal)
- Modify: PTY view component(s) under `crates/dux-web/web/src/` and `lib/store.ts` / `lib/eventsSocket.ts`
- Test: `crates/dux-web/src/server.rs` tests (owner-only input); web component test (placeholder + read-only)

**Interfaces:**
- Server: input (binary stdin) frames are applied only when the sending connection is the current sizing owner of that PTY (reuse `PtySizeOwners::may_resize` semantics, or a parallel `may_write`); a non-owner's stdin is dropped. On a new owner claim, emit an event `{event:"pty.owner", id:"<pty_id>"}` so other clients update their view.
- Client: on attaching a PTY view **while the tab is foregrounded** (Page Visibility `visibilityState === "visible"`), send the device's current size immediately (so the PTY snaps to fit). A backgrounded tab attaches as a non-owner observer and does **not** send size/claim. A non-owner view renders a placeholder ("Active on another device — Take over") and disables input; "Take over" re-attaches as owner (sends size, enables input).

- [ ] **Step 1: Write the failing server test (owner-only input)**

```rust
#[tokio::test]
async fn non_owner_stdin_is_dropped() {
    // two pty sockets on the same pty; conn B attaches last (owner)
    // conn A (non-owner) sends a stdin frame
    // assert: the PTY child does not receive A's bytes (only the owner's input is forwarded)
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p dux-web non_owner_stdin_is_dropped`
Expected: FAIL — today both connections' stdin is forwarded.

- [ ] **Step 3: Gate stdin on ownership**

In the PTY socket handler, before forwarding a binary stdin frame to `engine.write_pty`, check `pty_size_owners.may_resize(target.pty_id(), conn_id)` (owner check). If not the owner, drop the frame. (Resize is already owner-gated.) Emit a `pty.owner` event when a new connection claims ownership so other clients can flip to the placeholder.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p dux-web non_owner_stdin_is_dropped`
Expected: PASS.

- [ ] **Step 5: Frontend — foreground-aware ownership + size-on-attach**

In the PTY view component:
- On mount/attach, if `document.visibilityState === "visible"`, send the current terminal dimensions immediately after the socket opens (claim + fit). If hidden, do not send size (attach as observer).
- Track ownership from the `pty.owner` events: if another connection becomes owner, set this view to non-owner.

Add a web test asserting: a visible mount sends a size frame on open; a hidden mount does not.

- [ ] **Step 6: Frontend — placeholder + read-only secondary view (option B)**

When this view is a non-owner, render a placeholder card (shadcn/base-ui tokens, dark-only): "This session is active on another device." with a `Button` "Take over" (≥44px touch target). Disable the terminal input (read-only) while non-owner. "Take over" re-attaches as owner: sends size, re-enables input. Hover/secondary actions follow the `⋯`/SimpleTooltip conventions where applicable.

Add a web test: rendering with `isOwner=false` shows the placeholder and no editable terminal; clicking "Take over" triggers the re-attach path.

- [ ] **Step 7: Gates + commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test -p dux-web
( cd crates/dux-web/web && npx tsc -b && npx vitest run )
git add crates/dux-web/src/server.rs crates/dux-web/web/src/
git commit -m "Add a per-PTY active-owner model: most-recent foreground device drives size and input, others see a read-only take-over placeholder"
```

---

## Task 7: Connection registry, liveness ping, and notification-tag validation

**Files:**
- Modify: `crates/dux-web/src/server.rs` (add `ConnectionRegistry` to `AppState`; register/deregister on every socket connect/disconnect; liveness ping task)
- Modify: `crates/dux-web/src/rest_common.rs` (`scope_from_headers` validates against the registry)
- Test: `crates/dux-web/src/server.rs` tests; `crates/dux-web/src/rest_common.rs` tests

**Interfaces:**
- Produces: `ConnectionRegistry` — `{ insert(conn_id, class), remove(conn_id), contains(conn_id) -> bool, count(class) -> usize }`, thread-safe (`Mutex<HashMap<...>>`). Connection ids are the server-minted UUIDs already used for `/ws/events`; PTY sockets register too (for liveness + class counts).
- Changes: `scope_from_headers(headers, &registry) -> StatusScope` — returns `Connection(id)` only when `registry.contains(id)`, else `All`.

- [ ] **Step 1: Write the failing tag-validation test**

```rust
#[test]
fn unknown_connection_id_falls_back_to_all_scope() {
    let reg = ConnectionRegistry::default();
    let headers = header_map_with("x-connection-id", "does-not-exist");
    assert!(matches!(scope_from_headers(&headers, &reg), StatusScope::All));
}

#[test]
fn live_connection_id_scopes_to_that_connection() {
    let reg = ConnectionRegistry::default();
    reg.insert("conn-1".into(), ConnClass::Events);
    let headers = header_map_with("x-connection-id", "conn-1");
    assert!(matches!(scope_from_headers(&headers, &reg), StatusScope::Connection(id) if id == "conn-1"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-web connection_id_falls_back live_connection_id_scopes`
Expected: FAIL — `scope_from_headers` does not take a registry yet.

- [ ] **Step 3: Implement the registry + validation**

- Add `ConnectionRegistry` and store it in `AppState`.
- Register a connection id on each socket upgrade (events + both PTY types) with its class; deregister on disconnect.
- Change `scope_from_headers` to take `&ConnectionRegistry` and return `Connection(id)` only when `contains(id)`, else `All`. Update all mutation-handler call sites to pass the registry.

- [ ] **Step 4: Add the liveness ping**

Spawn a periodic task (wall-clock interval, e.g. every 30s) that sends a ping on each registered socket; a socket that fails/has not ponged within a grace window is closed and deregistered (freeing its permit + cap slot). Use the existing socket plumbing; do not block the engine thread.

- [ ] **Step 5: Run tests + gates**

Run: `cargo test -p dux-web ; cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/dux-web/src/server.rs crates/dux-web/src/rest_common.rs
git commit -m "Add a live-connection registry, liveness ping, and notification-tag validation that falls back to broadcasting on an unknown id"
```

---

## Task 8: Split the WebSocket connection cap into three classes

**Files:**
- Modify: `crates/dux-core/src/config.rs` (remove `max_websocket_connections`; add three fields + defaults + comments)
- Modify: `crates/dux-core/src/config_write.rs` (strip old key on every save; render three new keys)
- Modify: `crates/dux-web/src/server.rs` (three semaphores; events handler via helper; ~lines 82/314/637-652/767/834/1155)
- Modify: `crates/dux-web/src/engine_actor.rs` (`server_rebind_settings_changed` ~268; test ~1942)
- Modify: `crates/dux-tui/src/config.rs` (renderer ~650-663; test ~1292), `crates/dux-tui/src/cli.rs` (~315-317)
- Modify: `crates/dux-web/src/lib.rs`, `crates/dux-web/tests/auth_gate.rs` (field refs)
- Test: config back-compat + independence tests

**Interfaces:**
- Config: `max_websocket_events_connections` (default 32), `max_websocket_agent_connections` (default 32), `max_websocket_terminal_connections` (default 64), each `#[serde(default)]` with a per-field default const and a `=0 permanently blocks this class until restart` comment.
- `AppState`: three `Arc<Semaphore>` fields replacing `ws_semaphore`.

- [ ] **Step 1: Write failing config + independence tests**

```rust
// config.rs
#[test]
fn old_max_websocket_connections_key_still_loads_and_is_ignored() {
    let toml = r#"[server]
max_websocket_connections = 16
"#;
    let cfg: Config = toml::from_str(toml).unwrap(); // no error (no deny_unknown_fields)
    assert_eq!(cfg.server.max_websocket_events_connections, 32);
}
```
```rust
// server.rs
#[test]
fn one_class_saturated_does_not_block_another() {
    // terminal semaphore = 0, events semaphore = 1
    // assert: an events upgrade still acquires a permit
}
#[test]
fn permit_releases_on_drop() {
    // acquire from the terminal semaphore, assert available count drops; drop; assert it recovers
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-core -p dux-web max_websocket one_class_saturated permit_releases`
Expected: FAIL — fields/semaphores don't exist yet.

- [ ] **Step 3: Update `config.rs`**

Remove `max_websocket_connections` + its const; add:
```rust
const DEFAULT_MAX_WS_EVENTS: u32 = 32;
const DEFAULT_MAX_WS_AGENT: u32 = 32;
const DEFAULT_MAX_WS_TERMINAL: u32 = 64;
// in ServerConfig:
#[serde(default = "default_max_ws_events")]
pub max_websocket_events_connections: u32,
#[serde(default = "default_max_ws_agent")]
pub max_websocket_agent_connections: u32,
#[serde(default = "default_max_ws_terminal")]
pub max_websocket_terminal_connections: u32,
```
plus the three `fn default_*` helpers and the `Default` impl. **Note:** old configs carrying `max_websocket_connections` load fine because `ServerConfig` has no `#[serde(deny_unknown_fields)]` (TOML ignores unknown keys); this is *not* a `serde(default)` effect.

- [ ] **Step 4: Update the semaphores in `server.rs`**

Replace the single `ws_semaphore` field with `ws_events_semaphore`, `ws_agent_semaphore`, `ws_terminal_semaphore`; init from the three config values (~:314). Extend `acquire_ws_permit` to take the specific semaphore. **Refactor the events handler at ~:1155** (currently an inline `try_acquire_owned`) to call the helper with the events semaphore; PTY handlers (:767/:834) pass their own.

- [ ] **Step 5: Update `server_rebind_settings_changed` + its test**

In `engine_actor.rs:268`, replace the single comparison with three (OR'd); update the doc comment (~:252-258); split the test (~1942) into three per-field assertions.

- [ ] **Step 6: Update the canonical renderers + diff + remaining refs**

- `config_write.rs`: strip `max_websocket_connections` in the per-save patch path (mirror the oneshot strip at :491, which runs on every save) and render the three new commented keys.
- `dux-tui/src/config.rs`: replace the single `ConfigEntry::Field` (~650-663) with three commented entries; update the test assertion (~1292) to three.
- `dux-tui/src/cli.rs`: replace the single `diff_usize("server.max_websocket_connections", ...)` (~315-317) with three.
- Add a one-time startup log warning when the raw TOML still contains `max_websocket_connections`, naming the three replacements and noting `=0` meant "disable."
- `dux-web/src/lib.rs`, `tests/auth_gate.rs`: update field references.

- [ ] **Step 7: Run full gates**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test
( cd crates/dux-web/web && npx tsc -b && npx vitest run )
```
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add crates/dux-core/src/config.rs crates/dux-core/src/config_write.rs crates/dux-web/src/server.rs crates/dux-web/src/engine_actor.rs crates/dux-tui/src/config.rs crates/dux-tui/src/cli.rs crates/dux-web/src/lib.rs crates/dux-web/tests/auth_gate.rs
git commit -m "Split the WebSocket connection cap into separate events, agent, and terminal limits"
```

---

## Task 9: Change-gated spine check with a self-healing backstop

**Files:**
- Modify: `crates/dux-web/src/engine_actor.rs` (spine-check block ~1159-1178; bump sites at ~1345 apply, ~916 worker-drain, ~1107 foreground refresh, ~1112 prune; streaming counter in `poll_pty_activity` ~1102; `spine_fingerprints` ~1330 gets a `cfg(test)` call counter)
- Test: `engine_actor.rs` tests (with the required seams)

**Interfaces:**
- Internal actor state: `mutation_version: u64`, `streaming_version: u64`, `last_checked_mutation: u64`, `last_checked_streaming: u64`, `ticks_since_backstop: u32`.
- `#[cfg(test)]` `static SPINE_FP_CALLS: AtomicU64` incremented inside `spine_fingerprints`.
- `#[cfg(test)]` `EngineRequest::InjectSpineMutation` that renames a session without bumping `mutation_version`.

- [ ] **Step 1: Write the failing tests (with seams)**

```rust
#[test]
fn idle_ticks_do_not_serialize_the_spine() {
    // run N spine-check intervals with no commands/worker events/streaming change
    // assert: SPINE_FP_CALLS stays 0
}
#[test]
fn backstop_emits_a_change_that_bypassed_the_version() {
    // InjectSpineMutation (no version bump); advance past the backstop interval
    // assert: a sessions.changed event is emitted
}
#[test]
fn streaming_transition_triggers_a_check() {
    // run the engine directly; back-date pty_activity past AGENT_STREAMING_WINDOW
    // (mirror the existing hysteresis tests at engine/mod.rs:1639)
    // assert: streaming_version moved and the gate opened
}
#[test]
fn prune_exit_triggers_a_check_within_one_interval() {
    // a quiet agent exits; prune_exited_ptys returns non-empty -> version bump
    // assert: sessions.changed emitted within one spine-check interval (not the backstop)
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-web idle_ticks backstop_emits streaming_transition prune_exit`
Expected: FAIL — seams + gating don't exist.

- [ ] **Step 3: Add the seams**

- `#[cfg(test)] static SPINE_FP_CALLS: AtomicU64`; increment at the top of `spine_fingerprints`.
- `#[cfg(test)] EngineRequest::InjectSpineMutation { session_id, name }` handled by renaming the session directly (no version bump).

- [ ] **Step 4: Implement the gating + bumps + streaming counter + backstop**

- Bump `mutation_version` after each of the four loop mutators: `apply_wire` (~1345), `process_worker_event` (~916), and — only when they report a change — `refresh_terminal_foregrounds` (~1107) and `prune_exited_ptys` (~1112, bump when the returned `Vec` is non-empty).
- In `poll_pty_activity` (~1102), keep each agent's previous `is_agent_streaming()` value; bump `streaming_version` on any transition (O(1), no per-tick allocation).
- Rewrite the spine-check block (~1159-1178): run `spine_fingerprints` only if `mutation_version != last_checked_mutation || streaming_version != last_checked_streaming`, OR the backstop counter reached its interval (~40 ticks / ~2s). Update `last_checked_*` after each check; reset the backstop counter when it fires.
- Reframe the comment: the backstop is defense-in-depth for any future loop mutator added without a bump — not a claim that the bumps are exhaustive.

- [ ] **Step 5: Run tests + gates**

Run: `cargo test -p dux-web idle_ticks backstop_emits streaming_transition prune_exit ; cargo clippy --all-targets --all-features -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/dux-web/src/engine_actor.rs
git commit -m "Gate the spine serialize on actual change with a slow self-healing backstop"
```

---

## Task 10: Nest git/file REST routes under the session

**Files:**
- Modify: `crates/dux-web/src/git_routes.rs` (6 routes; structs lose `session_id`; add `Path<String>` + `id_within_bound`)
- Modify: `crates/dux-web/src/file_routes.rs` (6 routes; same)
- Modify: `crates/dux-web/src/server.rs` (access-log comment ~429-432)
- Modify: `crates/dux-web/web/src/lib/git.ts`, `lib/fileApi.ts`, `lib/markdown.ts`
- Modify: `crates/dux-web/web/src/lib/gitFileApi.test.ts`, `lib/markdown.test.ts`
- Test: Rust route tests (positive + negative); web vitest

**Interfaces:**
- Routes: `POST /api/v1/sessions/:id/git/{stage,unstage,discard,commit,push,pull}`; `POST /api/v1/sessions/:id/files/{list,read,diff,write,open-in-editor}`; `GET /api/v1/sessions/:id/files/raw?path=`. Each handler: `Path(id): Path<String>`, then `if !id_within_bound(&id) { return 404 }`, then `resolve_worktree(id)`.

- [ ] **Step 1: Update the Rust route tests first (red)**

Change existing tests to the nested paths + bodiless shapes, and add a positive test:
```rust
#[tokio::test]
async fn nested_git_stage_resolves_known_session() {
    // seeded engine with session s1; POST /api/v1/sessions/s1/git/stage {path:"f"}
    // assert: not a routing 404 (a worktree/other error is fine — proves :id extracted)
}
#[tokio::test]
async fn nested_git_unknown_session_is_404() { /* POST /api/v1/sessions/nope/git/stage -> 404 */ }
#[tokio::test]
async fn nested_git_oversized_id_is_404() { /* id length > MAX_ID_LEN -> 404 via id_within_bound */ }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p dux-web nested_git nested_file`
Expected: FAIL — old paths/struct shapes.

- [ ] **Step 3: Re-path the Rust handlers**

For each git/file route: change the path string to the nested form, replace the body `session_id` field with `Path(id): Path<String>`, add the `id_within_bound` guard at the top, and adjust bodies (commit `{message}`, push/pull bodiless, file ops keep `{path[, content, editor]}`, raw keeps `?path=`).

- [ ] **Step 4: Run to verify the Rust tests pass**

Run: `cargo test -p dux-web nested_git nested_file`
Expected: PASS.

- [ ] **Step 5: Update the frontend callers + their tests**

- `git.ts` (6): build `/api/v1/sessions/${id}/git/<action>`; drop `session_id` from bodies.
- `fileApi.ts` (5): build `/api/v1/sessions/${id}/files/<action>`; drop `session_id`.
- `markdown.ts:53`: `/api/v1/sessions/${encodeURIComponent(sessionId)}/files/raw?path=${encodeURIComponent(rel)}`.
- `gitFileApi.test.ts`: update URL + body assertions (e.g. commit body becomes `{ message }`).
- `markdown.test.ts:83`: assert the new raw URL shape.

- [ ] **Step 6: Fix the stale access-log comment**

In `server.rs:429-432`, update the example from `/api/file/raw?session_id=…` to `/api/v1/sessions/<id>/files/raw?path=…` and note the id is now an opaque path segment.

- [ ] **Step 7: Gates + commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test -p dux-web
( cd crates/dux-web/web && npx tsc -b && npx vitest run )
git add crates/dux-web/src/git_routes.rs crates/dux-web/src/file_routes.rs crates/dux-web/src/server.rs crates/dux-web/web/src/lib/
git commit -m "Nest git and file REST routes under /api/v1/sessions/:id and validate the id"
```

---

## Final verification (after all tasks)

- [ ] `cargo fmt`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test`
- [ ] `( cd crates/dux-web/web && npx tsc -b && npx vitest run )`
- [ ] Ask the user to `cargo run` and smoke-test: two-device same-session takeover (read-only placeholder on the displaced device, size snaps to the foreground device); reconnect leaves no stuck "loading changes"; saturating terminals does not block an events connection; adding/creating an agent in a detached-HEAD repo works.

## Docs to update (fold into the relevant task or a final docs commit)

- `website/docs/*` + `README.md` if they document the connection cap (now three keys) or the file/git API shape.
- The in-app `?` help / web UI copy if it references PTY ownership behavior (the new "active on another device" state).

## Punted (documented, not in this plan)

- The pre-existing client-supplied connection-id trust is addressed by Task 7 (validation → broadcast fallback); no further work.
- Strict no-refetch-on-same-rev (changing the client compare from `>=` to `>`) is out of scope; the one extra idempotent GET per reconnect is acceptable.
