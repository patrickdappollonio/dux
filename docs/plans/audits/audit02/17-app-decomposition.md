# Phase 17: App decomposition — split god-object into 6 sub-structs

> Maps to: **P1-V**.

## Goal
`src/app/mod.rs:54-201` declares 120 `pub(crate)` fields on `App`. Every
method in `app/{input,render,sessions,workers}.rs` reaches into `&mut
self` for any of those 120 fields — a peer-mutable god object. This
is the largest architectural smell; CLAUDE.md says new contributors
should be able to integrate with ease, but today they need to mentally
hold the entire field list before touching anything.

Decompose into 6 cohesive sub-structs held by `App`. **Pure refactor —
zero semantic change.** Effort: 2–3 days. **Largest single PR in audit02
— freeze other Rust PRs while it lands.**

## Pre-conditions
- Phase 00 baseline green.
- Phases 03 (sanitizer), 04 (workers), 05 (pty hardening), 09
  (tracing) MERGED — they touch the same code.
- Active PRs from other contributors paused for the duration.

## Files to touch
- `src/app/mod.rs` — define new sub-structs; rewire `App`.
- `src/app/{input,render,sessions,workers}.rs` — change `impl App` →
  `impl <SubStruct>` where natural; otherwise use thin shims.
- `src/app/state/` — NEW submodule directory with one file per sub-struct.

## Target structure
```rust
// src/app/mod.rs
pub struct App {
    pub(crate) ui: UiState,
    pub(crate) runtime: RuntimeState,
    pub(crate) git: GitState,
    pub(crate) remote: RemoteState,
    pub(crate) config: Config,
    pub(crate) theme: Theme,
}
```

### `UiState` — visual + input
- pane focus, scroll offsets, prompt stack
- mouse selection, palette, modal stack
- status line cache, render dirty flags
- terminal resize state

### `RuntimeState` — process + lifecycle
- PTY map (`HashMap<SessionId, PtyClient>`)
- worker channels (sender side)
- lockfile handle
- signal-hook registrations
- process startup time

### `GitState` — repo + worktree caches
- projects vec
- changed-files cache (per worktree)
- staged-diff cache
- branch state cache
- AI commit-message in-flight markers

### `RemoteState` — network
- `gh` auth state
- AMQ identity + queue depth cache
- PR sync state

### `Config` and `Theme` already exist — keep.

## Steps

### 17.1 — Inventory existing fields
```bash
cd src/app
awk '/^pub struct App/,/^}/' mod.rs | grep -c '^[[:space:]]*pub(crate)'
# expect ~120
```
Capture the list to `docs/plans/audits/audit02/artifacts/17-app-fields.txt`
as the baseline. Group each field into one of the 6 categories on paper
before touching code.

### 17.2 — Write the sub-structs (additive first)
Create `src/app/state/{ui.rs, runtime.rs, git.rs, remote.rs}` with
empty struct definitions. Add to `mod.rs`:
```rust
mod state;
pub(crate) use state::{UiState, RuntimeState, GitState, RemoteState};
```

### 17.3 — Migrate fields one category at a time
**Branch strategy**: one sub-struct per atomic commit. After each
commit, `cargo check && cargo clippy && cargo test` must be green.

For each sub-struct:
1. Move the relevant fields out of `App` into the sub-struct.
2. Add `App` field: `pub(crate) <name>: <SubStruct>`.
3. Replace every `self.<field>` reference across `app/*.rs` with
   `self.<sub_struct>.<field>` — use `sed` or rust-analyzer rename.
4. `cargo check` until clean. Resolve borrow-checker complaints by
   splitting borrows (`let App { ref mut ui, ref runtime, .. } = self;`).

### 17.4 — Move methods where natural
Some methods are clearly UI-only (e.g. `focus_left_pane`); move them
into `impl UiState`. Methods that touch ≥2 sub-structs stay on `App`
as orchestrators. Don't force-move methods just because — the goal is
field encapsulation, not method relocation.

### 17.5 — Lock split borrows
Many methods need both `&mut ui` and `&runtime`. Pattern:
```rust
fn render_focused_pane(&mut self) {
    let App { ui, runtime, theme, .. } = self;
    ui.render_pane(runtime.active_pty(), theme);
}
```
Avoid `&mut self` when split borrow works.

### 17.6 — Tests
- All existing tests must continue passing. No new tests required by
  the refactor itself.
- Add ONE smoke test that confirms `App` size has stayed roughly
  constant (no field forgotten):
  ```rust
  #[test]
  fn app_field_count_post_refactor() {
      // reflective field count, asserts UiState/RuntimeState/etc cover
      // the original 120 fields
  }
  ```
  (Optional — only if rustc supports trivial reflection via `derive`.)

## Validation
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test` green.
- `cargo check` time should not regress more than 10%; if it spikes,
  the split-borrow pattern is wrong somewhere.
- `wc -l src/app/mod.rs` — should drop from 2727 toward 1500 LOC; the
  rest moves to `state/*.rs`.
- Manual: launch dux, exercise every key path documented in the help
  overlay (`?` key). No regressions.

## Acceptance criteria
- [ ] `App` struct field count ≤ 8 (the 6 sub-structs + config + theme).
- [ ] `src/app/state/` exists with `{ui,runtime,git,remote}.rs`.
- [ ] Original 120 fields all migrated; none lost.
- [ ] `cargo clippy -D warnings` green.
- [ ] `cargo test` green.
- [ ] `wc -l src/app/mod.rs` substantially reduced.
- [ ] PR: `refactor(app): decompose god-object into UiState/RuntimeState/GitState/RemoteState (P1-V)`.

## Known pitfalls
- **Borrow checker is the main enemy.** Use struct-pattern destructuring
  to split borrows; resist `unsafe` workarounds.
- Some fields are conceptually shared (e.g. `last_render_time`).
  Place where they're most accessed; expose getter on `App` if needed.
- `pub(crate)` visibility — keep all fields `pub(crate)` for now;
  tighten in a follow-up after the dust settles.
- The refactor will conflict with EVERY in-flight Rust PR. Coordinate:
  freeze, land, then rebase others on top.
- Don't try to "improve" method bodies during the refactor — that's
  scope creep. Pure mechanical migration only.

## References
- audit02 P1-V.
- CLAUDE.md "App Module Structure" section.
- Prior precedent: rust-analyzer's `crate::Analysis` decomposition.
