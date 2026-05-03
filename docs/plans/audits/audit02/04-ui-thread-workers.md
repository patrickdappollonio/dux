# Phase 04: UI-thread unblocking — route startup git through workers

> Maps to: **P0-D** (CLAUDE.md tenet violation: blocking git on UI thread).

## Goal
Move every synchronous `Command::new("git")` call off the main event
loop. CLAUDE.md is explicit: even `git symbolic-ref` must use a worker.
Worst case today is `App::load_projects` looping over every configured
project and running `is_git_repo` + `current_branch` synchronously
before the first frame draws — N projects = N×fork+wait.

## Pre-conditions
- Phase 00 baseline green.
- Independent of Phases 01–03 and 05.

## Files to touch
- `src/app/workers.rs` — add new worker job kinds + dispatchers.
- `src/app/sessions.rs` — remove inline git calls.
- `src/app/mod.rs` — route `load_projects`, `reload_changed_files`
  through workers; add `WorkerEvent` variants.
- `src/app/input.rs` — route `staged_diff_text`, `commit` through workers.
- (no Cargo.toml changes; uses existing thread infra.)

## Sites to refactor (verified in audit02 P0-D)

| file:line                     | Current call                             |
|-------------------------------|------------------------------------------|
| `src/app/sessions.rs:50`      | `git::is_git_repo(&path)`                |
| `src/app/sessions.rs:66`      | `git::current_branch(&path)?`            |
| `src/app/sessions.rs:71`      | `git::remote_default_branch(&path)`      |
| `src/app/mod.rs:1974`         | `git::changed_files(&p)` in `reload_changed_files` |
| `src/app/mod.rs:2363`         | `git::is_git_repo(&p)` in `load_projects` (loop) |
| `src/app/mod.rs:2389`         | `git::current_branch(&path)` in `load_projects` (loop) |
| `src/app/input.rs:950`        | `git::staged_diff_text(&worktree)` before commit-msg worker |
| `src/app/input.rs:994`        | `git::commit(&worktree, &msg)` in execute_commit |
| `src/app/sessions.rs:577-582` | `git::remove_worktree` in `do_delete_session` (inline fallback) |

`src/app/input.rs:1039` (`git::is_dirty`) is already inside `thread::spawn`
— skip.

## Steps

### 4.1 — Add WorkerEvent variants
`src/app/workers.rs` — extend `WorkerEvent`:
```rust
pub(crate) enum WorkerEvent {
    // ... existing variants ...
    ProjectMetaReady {
        path: PathBuf,
        is_git: bool,
        current_branch: Option<String>,
        remote_default: Option<String>,
    },
    ChangedFilesReady { worktree: PathBuf, files: Vec<ChangedFile> },
    StagedDiffReady { worktree: PathBuf, diff: String },
    CommitFinished { worktree: PathBuf, result: anyhow::Result<()> },
    AddProjectMetaReady {
        path: PathBuf,
        result: anyhow::Result<ProjectMeta>,
    },
}
```

### 4.2 — Worker dispatchers
Add a new dispatch function in `workers.rs`:
```rust
pub(crate) fn dispatch_project_meta(
    tx: WorkerSender,
    paths: Vec<PathBuf>,
) {
    // Fan out one thread per project (small N, simple); collect on tx.
    for path in paths {
        let tx = tx.clone();
        std::thread::Builder::new()
            .name(format!("project-meta-{}", path.display()))
            .spawn(move || {
                let is_git = git::is_git_repo(&path);
                let current_branch = if is_git {
                    git::current_branch(&path).ok()
                } else { None };
                let remote_default = if is_git {
                    git::remote_default_branch(&path).ok()
                } else { None };
                let _ = tx.send(WorkerEvent::ProjectMetaReady {
                    path, is_git, current_branch, remote_default,
                });
            })
            .ok();
    }
}
```
Mirror for `dispatch_changed_files`, `dispatch_staged_diff`,
`dispatch_commit`, `dispatch_add_project_meta`.

### 4.3 — Replace call sites
At each site listed in the table, replace the inline call with a
dispatch + a `pending_*` flag on `App`. Render code displays a "loading"
placeholder until the corresponding `WorkerEvent::*Ready` lands and the
event handler in `drain_events` updates state.

Example for `load_projects` (`src/app/mod.rs:2363,2389`):
```rust
fn load_projects(&mut self) {
    let paths: Vec<_> = self.config.projects.iter()
        .map(|p| p.path.clone()).collect();
    // Don't block — fan out and let drain_events fill in metadata.
    self.projects = paths.iter().map(|p| Project::placeholder(p.clone())).collect();
    workers::dispatch_project_meta(self.worker_tx.clone(), paths);
}

// in drain_events:
WorkerEvent::ProjectMetaReady { path, is_git, current_branch, remote_default } => {
    if let Some(proj) = self.projects.iter_mut().find(|p| p.path == path) {
        proj.is_git = is_git;
        proj.current_branch = current_branch;
        proj.remote_default = remote_default;
        proj.meta_loaded = true;
    }
}
```

`Project::placeholder` is a stub showing path + "loading…" until the
event arrives.

### 4.4 — Render placeholders
In `src/app/render.rs`, wherever `Project` fields are read, add
`if !p.meta_loaded { "(loading…)" } else { … }`. Avoids panics on
half-populated state.

## Validation
- `cargo test` — add a unit test that asserts `App::new` returns within
  100 ms even when `git` is `sleep 5` (fake binary in PATH for the test).
- Manual: configure 10 projects in `config.toml`; restart dux; first
  frame should draw within 200 ms (was previously N×fork-wait).
- `cargo clippy --all-targets -- -D warnings` green.

## Acceptance criteria
- [ ] All 9 sites in the table no longer call `git::*` directly on UI thread.
- [ ] `WorkerEvent` has 5 new variants; `drain_events` handles each.
- [ ] `App::new` benchmarks under 100 ms with 10 placeholder projects.
- [ ] No new panics; placeholder render path covers all loading states.
- [ ] PR: `perf(app): move blocking git off UI thread (P0-D)`.

## Known pitfalls
- `commit` is order-sensitive (user may type into prompt mid-commit).
  Block input during commit using existing `PromptState` machinery, not
  by re-blocking the UI thread.
- `changed_files` is called frequently (selection change). Coalesce via
  a debounce (200 ms) to avoid worker thrash on rapid pane navigation.
  Use the existing `spawn_changed_files_poller` rather than a new worker.
- `do_delete_session` has an inline-fallback path at `:577-582` for
  legacy code. The async path via `begin_delete_session` already exists
  — deprecate the inline path entirely.
- `App::new` cannot easily await a worker for first-paint; placeholders
  are the right move. Don't try to make `load_projects` "block briefly".

## References
- CLAUDE.md tenet: "Long-running actions should not block the UI thread."
- audit02 P0-D.
