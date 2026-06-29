# Configurable Instance Title Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an operator name a dux web instance (e.g. `dux #1`) via one config value that drives both the browser tab `<title>` and the brand wordmark in the projects pane (desktop sidebar and mobile drawer), keeping the version line directly below it.

**Architecture:** Add a single `title` string to `[server]` config (web-only branding). Project it through the existing `BootstrapView` (`GET /api/v1/bootstrap`) so it reaches the browser on load and re-arrives on every `config.changed` refetch. A pure frontend helper `resolveInstanceTitle()` resolves the raw value (trim, fall back to `"dux"`); the store's single bootstrap-apply choke point sets `document.title`, and the two brand-wordmark components render the same resolved value.

**Tech Stack:** Rust (`serde`, `toml`, `toml_edit`) for config + the bootstrap projection; React + TypeScript + Vite (Vitest) for the web frontend; existing zustand-style external store in `crates/dux-web/web/src/lib/store.ts`.

## Global Constraints

- **All settings are configurable and the config file is the documentation.** The new field MUST ship with an inline `CommentSource::Static` comment in the canonical schema that explains what it does, including the multi-instance use case. (CLAUDE.md: Configuration tenets.)
- **Two config renderers must stay in sync.** Every managed config field needs (a) a `ConfigEntry::Field` in the commented schema `config_schema()` in `crates/dux-tui/src/config.rs` AND (b) a `patch_*` call in `apply_patches()` in `crates/dux-core/src/config_write.rs`. Forgetting (b) silently drops the field on a plain/recover write — this is the documented "[server] lesson" in `config_write.rs`.
- **Web UI is dark-only and styled through shadcn/base-ui token classes.** No hardcoded colors; reuse the existing wordmark/version markup and token classes (`font-semibold`, `text-sidebar-foreground/70`, `text-muted-foreground`). (CLAUDE.md: Web UI conventions.)
- **No byte-based truncation of user-visible strings.** Display truncation is done with CSS (`truncate` + `min-w-0`), never JS string slicing.
- **Prove the work with tests.** Pure logic (`resolveInstanceTitle`, config parse/round-trip, bootstrap projection) is unit-tested. Rust CI gate: `cargo clippy --all-targets --all-features -- -D warnings` must pass with zero warnings.
- **Auth is being removed by a parallel agent.** Do NOT touch `LoginScreen.tsx`, the `App.tsx` auth-unreachable screen, or any auth code. The chosen `document.title` site (`applyBootstrap` in `store.ts`) deliberately avoids editing `App()` to stay clear of that work.
- **Verification commands.** Rust (repo root): `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`. Frontend (from `crates/dux-web/web/`): `npm run test`, `npm run lint`, `npm run build`.

---

## File Structure

Files created or modified, by responsibility:

- `crates/dux-core/src/config.rs` — **Modify.** Add `ServerConfig.title: String` field + doc comment; add to `Default`; add a parse/default unit test.
- `crates/dux-tui/src/config.rs` — **Modify.** Add the commented `ConfigEntry::Field` for `title` to `config_schema()`; assert it in the existing render test.
- `crates/dux-core/src/config_write.rs` — **Modify.** Add the `patch_table_str` call for `title` in `apply_patches()`; extend the plain round-trip test.
- `crates/dux-core/src/viewmodel.rs` — **Modify.** Add `BootstrapView.title: String`, project it in `Engine::bootstrap()`, add a projection test, and add `"title"` to the JSON-fields test.
- `crates/dux-web/web/src/lib/bootstrapApi.ts` — **Modify.** Add `title?: string` (optional for back-compat) to the `Bootstrap` interface.
- `crates/dux-web/web/src/lib/instanceTitle.ts` — **Create.** Pure `resolveInstanceTitle()` helper + the `"dux"` fallback constant.
- `crates/dux-web/web/src/lib/instanceTitle.test.ts` — **Create.** Unit tests for the helper.
- `crates/dux-web/web/src/lib/store.ts` — **Modify.** In `applyBootstrap()`, set `document.title` (DOM-guarded) from the resolved title.
- `crates/dux-web/web/src/lib/storeBootstrap.test.ts` — **Modify.** Stub `document` and assert `applyBootstrap` sets the tab title.
- `crates/dux-web/web/src/components/Sidebar.tsx` — **Modify.** Render the resolved title in the desktop brand block; keep version below; truncate.
- `crates/dux-web/web/src/components/MobileShell.tsx` — **Modify.** Render the resolved title in the mobile drawer header; keep the "agent sessions" subtitle below; truncate.

---

### Task 1: `[server] title` config field (Rust core + commented schema)

**Files:**
- Modify: `crates/dux-core/src/config.rs` (struct `ServerConfig` ~156-208, `impl Default for ServerConfig` ~406-421, tests module ~2484)
- Modify: `crates/dux-tui/src/config.rs` (`config_schema()` server fields ~650-664, render test ~1292)
- Modify: `crates/dux-core/src/config_write.rs` (`apply_patches()` server block ~302-309, round-trip test ~810-845)

**Interfaces:**
- Consumes: nothing new.
- Produces: `config.server.title: String` (default `"dux"`), serialized/parsed as `[server].title`, rendered with an inline comment, round-tripped through the plain writer. Later tasks read `config.server.title`.

- [ ] **Step 1: Write the failing parse/default test**

In `crates/dux-core/src/config.rs`, inside the existing `#[cfg(test)] mod tests` block (next to `server_config_parses_full_section`), add:

```rust
#[test]
fn server_title_defaults_to_dux_and_parses_override() {
    // No [server] section: title defaults to the product name.
    let default: Config = toml::from_str("").expect("empty config should parse");
    assert_eq!(default.server.title, "dux");

    // An explicit title (e.g. to tell multiple instances apart) round-trips.
    let config: Config = toml::from_str(
        r#"
[server]
title = "dux #1"
"#,
    )
    .expect("config with [server] title should parse");
    assert_eq!(config.server.title, "dux #1");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p dux-core server_title_defaults_to_dux_and_parses_override`
Expected: FAIL to compile — `no field 'title' on type 'ServerConfig'`.

- [ ] **Step 3: Add the field to `ServerConfig` with a doc comment**

In `crates/dux-core/src/config.rs`, inside `pub struct ServerConfig`, add the field immediately after `pub max_websocket_connections: u32,` (before `pub acme: AcmeSettings,`):

```rust
    /// WEB-ONLY display name for this dux instance. Drives the browser tab
    /// `<title>` and the brand wordmark in the web projects pane (the version
    /// line stays directly below it). Set a distinct value per instance — e.g.
    /// "dux #1" / "dux (prod)" — to tell several dux tabs apart at a glance.
    /// Default "dux". An empty/whitespace value falls back to "dux" in the UI.
    pub title: String,
```

- [ ] **Step 4: Add the default**

In `impl Default for ServerConfig`, add the field to the returned struct (after `max_websocket_connections: ...,` and before `acme: AcmeSettings::default(),`):

```rust
            title: "dux".to_string(),
```

- [ ] **Step 5: Run the parse test to verify it passes**

Run: `cargo test -p dux-core server_title_defaults_to_dux_and_parses_override`
Expected: PASS.

- [ ] **Step 6: Add the commented schema entry (canonical first-creation renderer)**

In `crates/dux-tui/src/config.rs`, in `config_schema()`, add a new `ConfigEntry::Field` for `title` immediately after the `max_websocket_connections` field entry and before `ConfigEntry::Blank` (i.e. right after the block ending `value_fn: |c| FieldValue::Usize(c.server.max_websocket_connections as usize),` + its closing `},`):

```rust
        ConfigEntry::Field {
            key: "title",
            comment: Some(CommentSource::Static(
                "# Display name for THIS dux instance in the web UI. It is shown as\n\
                 # the browser tab title and as the brand wordmark at the top of the\n\
                 # projects pane (the version stays on the line below). Give each\n\
                 # instance a distinct value — for example \"dux #1\" or \"dux (prod)\"\n\
                 # — so several dux tabs/servers are easy to tell apart. An empty or\n\
                 # whitespace-only value falls back to \"dux\".",
            )),
            value_fn: |c| FieldValue::Str(c.server.title.clone()),
        },
```

- [ ] **Step 7: Assert the rendered default config contains the field**

In `crates/dux-tui/src/config.rs`, in the render test that asserts the `[server]` section (the one containing `assert!(rendered.contains("max_websocket_connections = 128"));`), add directly after that line:

```rust
        assert!(rendered.contains("title = \"dux\""));
```

- [ ] **Step 8: Add the plain-writer patch call**

In `crates/dux-core/src/config_write.rs`, in `apply_patches()`, in the `// --- [server] ---` block, add after the `max_websocket_connections` patch call (after the closing `);` of `patch_table_usize(...)` and before `// --- [server.acme] ---`):

```rust
    patch_table_str(doc, "server", "title", &config.server.title);
```

- [ ] **Step 9: Extend the plain round-trip test**

In `crates/dux-core/src/config_write.rs`, in `write_config_plain_round_trips_server_section`, add a mutation alongside the others (after `config.server.max_websocket_connections = 42;`):

```rust
        config.server.title = "dux #1".to_string();
```

and an assertion after `assert_eq!(parsed.server.max_websocket_connections, 42);`:

```rust
        assert_eq!(parsed.server.title, "dux #1");
```

- [ ] **Step 10: Run the full config test surface**

Run: `cargo test -p dux-core config && cargo test -p dux-tui config`
Expected: PASS (parse/default, plain round-trip, and the render-contains-`title` assertion all green).

- [ ] **Step 11: Lint + format**

Run: `cargo fmt && cargo clippy -p dux-core -p dux-tui --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 12: Commit**

```bash
git add crates/dux-core/src/config.rs crates/dux-tui/src/config.rs crates/dux-core/src/config_write.rs
git commit -m "Add a configurable [server] title for the web instance name"
```

---

### Task 2: Project `title` through the bootstrap view

**Files:**
- Modify: `crates/dux-core/src/viewmodel.rs` (`struct BootstrapView` ~31-85, `Engine::bootstrap()` ~362-394, tests ~865-964)

**Interfaces:**
- Consumes: `config.server.title` (Task 1).
- Produces: `BootstrapView.title: String`, serialized as `"title"` in the `GET /api/v1/bootstrap` JSON. The frontend consumes it in Task 3.

- [ ] **Step 1: Write the failing projection test**

In `crates/dux-core/src/viewmodel.rs`, in the `#[cfg(test)] mod` (next to `status_clear_seconds_is_projected`), add:

```rust
#[test]
fn server_title_is_projected() {
    let (mut engine, _tmp) = test_engine();
    // Defaults flow through unchanged.
    assert_eq!(engine.bootstrap().title, "dux");
    // A configured instance name reaches the bootstrap view verbatim (the web
    // resolves empty/whitespace to "dux"; the projection itself is faithful).
    engine.config.server.title = "dux #1".to_string();
    assert_eq!(engine.bootstrap().title, "dux #1");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p dux-core server_title_is_projected`
Expected: FAIL to compile — `no field 'title' on type 'BootstrapView'` (and the `BootstrapView { ... }` literal in `bootstrap()` errors as soon as the field is added without being set, which Step 4 resolves).

- [ ] **Step 3: Add the field to `BootstrapView`**

In `crates/dux-core/src/viewmodel.rs`, in `pub struct BootstrapView`, add after `pub status_clear_seconds: u16,`:

```rust
    /// Mirrors `config.server.title`: the operator-chosen display name for this
    /// dux instance. The web shows it as the browser tab title and the brand
    /// wordmark above the version in the projects pane, and resolves an
    /// empty/whitespace value to "dux". Older servers omit it (the web treats a
    /// missing value as "dux").
    pub title: String,
```

- [ ] **Step 4: Populate it in `Engine::bootstrap()`**

In the `BootstrapView { ... }` literal returned by `bootstrap()`, add after `status_clear_seconds: self.config.ui.status_clear_seconds,`:

```rust
            title: self.config.server.title.clone(),
```

- [ ] **Step 5: Run the projection test to verify it passes**

Run: `cargo test -p dux-core server_title_is_projected`
Expected: PASS.

- [ ] **Step 6: Add `title` to the JSON-fields guard test**

In `crates/dux-core/src/viewmodel.rs`, in `bootstrap_serializes_to_json_with_expected_fields`, add `"title"` to the array of field names being asserted (after `"global_env",`). While here, also add `"status_clear_seconds"` — it is a real serialized `BootstrapView` field the array currently omits, so closing that pre-existing gap costs nothing and keeps the guard honest:

```rust
            "title",
            "status_clear_seconds",
```

Note: this guard uses a substring match (`json.contains("\"title\"")`), so it proves the key is present but not its value or exact name. The faithful value/projection is what `server_title_is_projected` (Step 1) actually verifies; this array is only a cheap "field didn't silently disappear" tripwire, consistent with the existing test style.

- [ ] **Step 7: Run the JSON guard test**

Run: `cargo test -p dux-core bootstrap_serializes_to_json_with_expected_fields`
Expected: PASS.

- [ ] **Step 8: Lint + format**

Run: `cargo fmt && cargo clippy -p dux-core --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/dux-core/src/viewmodel.rs
git commit -m "Project the server title through the web bootstrap view"
```

---

### Task 3: `resolveInstanceTitle` helper + bootstrap type field

**Files:**
- Create: `crates/dux-web/web/src/lib/instanceTitle.ts`
- Create: `crates/dux-web/web/src/lib/instanceTitle.test.ts`
- Modify: `crates/dux-web/web/src/lib/bootstrapApi.ts` (`interface Bootstrap` ~18-48)

**Interfaces:**
- Consumes: `BootstrapView.title` JSON (Task 2), surfaced as `Bootstrap.title?: string`.
- Produces:
  - `DEFAULT_INSTANCE_TITLE: string` (= `"dux"`)
  - `resolveInstanceTitle(raw: string | null | undefined): string` — trims `raw`; returns `DEFAULT_INSTANCE_TITLE` when empty/whitespace/missing, otherwise the trimmed value.
  - `Bootstrap.title?: string`
  Tasks 4 and 5 import `resolveInstanceTitle` (and `DEFAULT_INSTANCE_TITLE` where a fallback is needed).

- [ ] **Step 1: Write the failing helper test**

Create `crates/dux-web/web/src/lib/instanceTitle.test.ts`:

```ts
import { describe, expect, it } from "vitest"

import { DEFAULT_INSTANCE_TITLE, resolveInstanceTitle } from "./instanceTitle"

describe("resolveInstanceTitle", () => {
  it("returns a configured title verbatim", () => {
    expect(resolveInstanceTitle("dux #1")).toBe("dux #1")
  })

  it("trims surrounding whitespace", () => {
    expect(resolveInstanceTitle("  dux (prod)  ")).toBe("dux (prod)")
  })

  it("falls back to the product name when missing", () => {
    expect(resolveInstanceTitle(undefined)).toBe(DEFAULT_INSTANCE_TITLE)
    expect(resolveInstanceTitle(null)).toBe(DEFAULT_INSTANCE_TITLE)
  })

  it("falls back when empty or whitespace only", () => {
    expect(resolveInstanceTitle("")).toBe("dux")
    expect(resolveInstanceTitle("   ")).toBe("dux")
  })

  it("collapses internal newlines so the tab and wordmark stay identical", () => {
    // A hand-edited config can contain a TOML newline escape; browsers truncate a
    // tab title at the first newline while a nowrap span would show a space.
    expect(resolveInstanceTitle("dux\nlab")).toBe("dux lab")
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run (from `crates/dux-web/web/`): `npm run test -- instanceTitle`
Expected: FAIL — cannot resolve `./instanceTitle`.

- [ ] **Step 3: Write the helper**

Create `crates/dux-web/web/src/lib/instanceTitle.ts`:

```ts
/** Product fallback used when no instance title is configured (or it is blank). */
export const DEFAULT_INSTANCE_TITLE = "dux"

/**
 * Resolve the operator-configured instance title (`config.server.title`, carried
 * on the bootstrap document) into the string the UI should display. Collapses any
 * internal CR, LF, or CRLF sequence to a single space and trims surrounding
 * whitespace, then falls back to {@link DEFAULT_INSTANCE_TITLE} when the value is
 * missing, empty, or whitespace-only. Used for both the browser tab title and the brand wordmark so
 * the two surfaces never drift (browsers truncate a tab title at a newline, so
 * the collapse keeps the tab and the wordmark identical).
 */
export function resolveInstanceTitle(raw: string | null | undefined): string {
  const normalized = (raw ?? "").replace(/[\r\n]+/g, " ").trim()
  return normalized === "" ? DEFAULT_INSTANCE_TITLE : normalized
}
```

- [ ] **Step 4: Run the helper test to verify it passes**

Run (from `crates/dux-web/web/`): `npm run test -- instanceTitle`
Expected: PASS (5 tests).

- [ ] **Step 5: Add the optional field to the `Bootstrap` type**

In `crates/dux-web/web/src/lib/bootstrapApi.ts`, in `export interface Bootstrap`, add after the `status_clear_seconds` field:

```ts
  /** The operator-chosen display name for this dux instance (`config.server
   * .title`). Shown as the browser tab title and the projects-pane wordmark.
   * Optional: older servers omit it, so consumers resolve a missing/blank value
   * to "dux" via `resolveInstanceTitle`. */
  title?: string
```

Also fix the file-level comment just above `export interface Bootstrap`. It currently asserts a false invariant ("Every field is required: the server always projects all of them.") — false because `title?` is optional, and already loosely false because `status_clear_seconds`'s own doc says older servers omit it. Replace the WHOLE three-line block (do not edit only the last sentence, or you'll produce a 200-char line and drop the useful first two lines) with:

```ts
// The bootstrap document. Field names/types mirror the server's JSON (snake_case)
// and the values the legacy ViewModel carried, so consumers move over without a
// shape change. Newer fields may be absent when talking to an older server (a `?`
// marks the ones typed optional, e.g. `title`); consumers fall back to the
// per-field documented default rather than assuming every field is present.
```

(Deliberately NOT changing `status_clear_seconds: number` to optional here — that is a pre-existing type/doc mismatch in someone else's field and is out of scope for this change. The reworded comment is accurate for it regardless, since it no longer ties absence strictly to the `?` marker.)

- [ ] **Step 6: Typecheck**

Run (from `crates/dux-web/web/`): `npm run build`
Expected: `tsc -b` passes (no type errors); Vite build succeeds.

- [ ] **Step 7: Commit**

```bash
git add crates/dux-web/web/src/lib/instanceTitle.ts crates/dux-web/web/src/lib/instanceTitle.test.ts crates/dux-web/web/src/lib/bootstrapApi.ts
git commit -m "Add resolveInstanceTitle helper and bootstrap title field"
```

---

### Task 4: Set `document.title` from the bootstrap apply choke point

**Files:**
- Modify: `crates/dux-web/web/src/lib/store.ts` (`applyBootstrap()` ~663-672, imports ~31)
- Modify: `crates/dux-web/web/src/lib/storeBootstrap.test.ts` (add a nested `describe`; do NOT touch the file-wide `beforeEach`)

**Interfaces:**
- Consumes: `resolveInstanceTitle` (Task 3), `Bootstrap.title` (Task 3).
- Produces: a side effect — `document.title` reflects the configured instance title after every bootstrap apply (first load and `config.changed` refetch). No new exports.

**Why here (not `App.tsx`):** `applyBootstrap` is the single place bootstrap lands (verified: it is the only apply site, and `config.changed` routes through it via `loadBootstrap`), so one write covers both first load and live config reloads, and it keeps this change out of `App()` while the auth-removal agent edits that file. The write is DOM-guarded because the store runs in a Node test environment (`storeBootstrap.test.ts` stubs `window`/`location`/etc. but **not** `document`).

> **Sequencing note (auth removal):** the existing `loadStore()` helper waits on `auth.phase`, and bootstrap is fetched today inside `bootAuth()`. A parallel agent is removing auth, which will rewrite the boot sequence and likely `loadStore()` itself. Implement this task **after** auth removal has landed (or coordinate), and mirror whatever boot/wait helper the neighbouring passing tests use at that time — do not hardcode against today's `auth.phase`-based `loadStore()` if it has changed.

- [ ] **Step 1: Write the failing test**

Do **not** call `bootAuth()` from the test — it is a private, non-exported function in `store.ts` (the store self-boots via a bare `void bootAuth()` at module load). The file's existing `loadStore()` helper (`storeBootstrap.test.ts:92`) imports the store and `await vi.waitFor(...)`s until `bootstrap` is non-null — i.e. until `applyBootstrap` has run. Reuse it.

Add a dedicated nested `describe` at the end of the top-level `describe("bootstrap slice", ...)`. Give it its OWN `beforeEach` that stubs `document` so the stub is scoped to these tests only — the file-wide `beforeEach` must stay document-free so the `typeof document` guard stays meaningful (and exercised as "absent") for every other test. Initialize the stub title to a sentinel, not `"dux"`, so a pass proves `applyBootstrap` actually wrote the value rather than coincidentally matching the stub's initial state:

```ts
describe("instance title → document.title", () => {
  beforeEach(() => {
    vi.stubGlobal("document", { title: "pending" })
  })

  it("sets document.title from the configured instance title", async () => {
    bootstrapBody = makeBootstrap({ title: "dux #1" })
    await loadStore()
    expect(document.title).toBe("dux #1")
  })

  it("resolves a blank instance title to the product name", async () => {
    bootstrapBody = makeBootstrap({ title: "   " })
    await loadStore()
    expect(document.title).toBe("dux")
  })
})
```

(`loadStore()` already waits for the slice to populate, so no extra `vi.waitFor` is needed. The file's outer `afterEach` runs `vi.unstubAllGlobals()`, which clears the `document` stub between tests; Vitest runs the outer `beforeEach` before this nested one, so all the other stubs are still in place.)

- [ ] **Step 2: Run the test to verify it fails**

Run (from `crates/dux-web/web/`): `npm run test -- storeBootstrap`
Expected: FAIL — `document.title` stays `"pending"` (the store does not set it yet).

- [ ] **Step 3: Import the helper in the store**

In `crates/dux-web/web/src/lib/store.ts`, add an import near the existing `bootstrapApi` import (line ~31):

```ts
import { resolveInstanceTitle } from "./instanceTitle"
```

- [ ] **Step 4: Set the title in `applyBootstrap`**

In `applyBootstrap(b: Bootstrap)`, after the existing `setState({ ... })` call and before the function closes, add:

```ts
  // Reflect the configured instance name in the browser tab. Guarded because the
  // store also runs under the Node test environment, where `document` is absent
  // unless a test stubs it. Runs on first load and on every config.changed
  // refetch, so a live rename updates the tab without a reload.
  if (typeof document !== "undefined") {
    document.title = resolveInstanceTitle(b.title)
  }
```

- [ ] **Step 5: Run the test to verify it passes**

Run (from `crates/dux-web/web/`): `npm run test -- storeBootstrap`
Expected: PASS (both new cases plus the pre-existing bootstrap tests).

- [ ] **Step 6: Typecheck + lint**

Run (from `crates/dux-web/web/`): `npm run build && npm run lint`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add crates/dux-web/web/src/lib/store.ts crates/dux-web/web/src/lib/storeBootstrap.test.ts
git commit -m "Drive the browser tab title from the configured instance name"
```

---

### Task 5: Render the instance title in the projects-pane wordmark (desktop + mobile)

**Files:**
- Modify: `crates/dux-web/web/src/components/Sidebar.tsx` (brand block ~787-801, imports)
- Modify: `crates/dux-web/web/src/components/MobileShell.tsx` (drawer header ~557-562, imports)

**Interfaces:**
- Consumes: `resolveInstanceTitle` (Task 3), `bootstrap.title` (Task 3). `bootstrap` is **already** destructured from `useDux()` in `Sidebar.tsx` (`AppSidebar`), but is **NOT** in scope in the mobile `HomeScreen` component — Step 2 adds it there.
- Produces: no exports — JSX wiring only.

> **Accepted coverage gap:** these components have no unit-test harness in this repo (there are zero `*.test.tsx` files and no `@testing-library/react`/jsdom dependency), so the wordmark render is verified by typecheck/lint/build plus the manual smoke step — not an automated render test. TypeScript will catch a missing import or wrong type but NOT a wrong-field mistake (e.g. passing `bootstrap?.dux_version` instead of `bootstrap?.title`, both `string | undefined`). Adding a React render harness is out of scope for this plan; the owning engineer accepts this gap consciously. The pure resolver these components call is fully unit-tested in Task 3.

- [ ] **Step 1: Update the desktop sidebar brand block**

In `crates/dux-web/web/src/components/Sidebar.tsx`, add the helper import next to the other `@/lib/...` imports:

```ts
import { resolveInstanceTitle } from "@/lib/instanceTitle"
```

`bootstrap` is already destructured from `useDux()` in this component. Replace the brand block (currently):

```tsx
              <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
              <div className="flex flex-1 flex-col gap-0.5 leading-none">
                <span className="font-semibold">dux</span>
                <span className="text-sm text-sidebar-foreground/70">
                  {bootstrap?.dux_version}
                </span>
              </div>
```

First compute the title once, just above the component's `return` (so a single value feeds the wordmark and stays trivially in sync):

```tsx
  const instanceTitle = resolveInstanceTitle(bootstrap?.title)
```

Then replace the brand block with (note: the logo `alt` stays `"dux"` — the image is decorative and the adjacent wordmark already announces the name to screen readers; re-labelling the `alt` would only duplicate it):

```tsx
              <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
              <div className="flex min-w-0 flex-1 flex-col gap-0.5 leading-none">
                <span className="truncate font-semibold">{instanceTitle}</span>
                <span className="text-sm text-sidebar-foreground/70">
                  {bootstrap?.dux_version}
                </span>
              </div>
```

(`SidebarMenuButton` already clips with `overflow-hidden`, so a long title can never break out of the sidebar. The point of `min-w-0` on the column + `truncate` on the wordmark is to let the text shrink and render an **ellipsis** instead of being hard-clipped. The version line is unchanged and stays directly below.)

- [ ] **Step 2: Update the mobile drawer header**

In `crates/dux-web/web/src/components/MobileShell.tsx`, add the import next to the other `@/lib/...` imports:

```ts
import { resolveInstanceTitle } from "@/lib/instanceTitle"
```

The drawer header is rendered by the inner `HomeScreen` component (`function HomeScreen()` ~line 526), **not** the exported `MobileShell` (~line 785, which only reads `mobileScreen` and routes to `<HomeScreen />`). `HomeScreen`'s `useDux()` destructure does **not** currently include `bootstrap`, so this is a REQUIRED edit, not a check. Add `bootstrap` to it:

```tsx
  const {
    spine,
    bootstrap,
    selectedTarget,
    pendingSessionOrder,
    pendingProjectOrder,
    auth,
  } = useDux()
```

Compute the title once near the top of `HomeScreen`'s body (after the destructure):

```tsx
  const instanceTitle = resolveInstanceTitle(bootstrap?.title)
```

Then replace the wordmark span in the drawer header (currently `<span className="font-semibold">dux</span>` at ~560) with — leaving the logo `alt="dux"` and the "agent sessions" subtitle line unchanged:

```tsx
        <span className="truncate font-semibold">{instanceTitle}</span>
```

The header's column already has `min-w-0 flex-1`, so `truncate` ellipsizes a long title there too.

- [ ] **Step 3: Typecheck + lint**

Run (from `crates/dux-web/web/`): `npm run build && npm run lint`
Expected: clean (no unused-import or type errors).

- [ ] **Step 4: Manual smoke (record result, do not block on tooling)**

With a config containing `[server]\ntitle = "dux #1"`, run the server and confirm: the browser tab reads `dux #1`, the desktop sidebar wordmark reads `dux #1` with the version directly below, and the mobile drawer header reads `dux #1` with "agent sessions" below. With no `title` set (or blank), all three read `dux`.

- [ ] **Step 5: Commit**

```bash
git add crates/dux-web/web/src/components/Sidebar.tsx crates/dux-web/web/src/components/MobileShell.tsx
git commit -m "Show the configured instance title in the web projects pane"
```

---

## Final Verification

- [ ] Run `cargo fmt && cargo clippy --all-targets --all-features -- -D warnings && cargo test` from the repo root — all green, zero warnings.
- [ ] Run `npm run test && npm run lint && npm run build` from `crates/dux-web/web/` — all green.
- [ ] Grep check: `git grep -n 'font-semibold">dux<'` returns nothing (no hardcoded wordmark remains in the two brand blocks).

---

## Out of Scope / Punted (low-priority — remember, do not silently drop)

These were considered and deliberately deferred. They are NOT required for this plan to be complete, but should not be forgotten:

- **Configurable favicon / monochrome icon.** The original ask also wanted a configurable, monochrome favicon. That is a separate feature (a new config field, a dynamic `<link rel="icon">` swap, and possibly a server route to serve a file-path icon, plus a monochrome default asset). It is intentionally excluded here so the title feature ships independently. Track it as its own plan.
- **Disconnect blur/grayscale overlay.** The other half of the original ask (full-screen blur + grayscale + reconnect modal on connection loss) is a wholly separate feature with its own plan; not touched here.
- **`Welcome.tsx` logo `aria-label="dux"`** (and the decorative `alt="dux"` on the sidebar/mobile logos). These stay as the static product name: the logo is decorative and the adjacent wordmark already carries the instance name for screen readers, so re-labelling would only duplicate it. Not required; revisit only if the wordmark is ever removed.
- **PWA manifest `name`/`short_name`** (`web/public/manifest.webmanifest`) stays `dux`. **This is a genuine gap, not a non-issue:** when two instances are installed as PWAs, both launcher icons read `dux`, which is exactly the multi-instance confusion the title feature exists to solve. It is excluded here only to keep this plan to the explicitly-requested surfaces (tab title + projects pane). **Recommended immediate follow-up:** add a small dynamic-manifest route in `dux-web` — a `GET /manifest.webmanifest` handler that serves a serde-serialized body substituting `config.server.title` into `name`/`short_name` — and drop the static file from `rust-embed`. It is a single handler, so it is in proportion; it is simply a separate change. Track it as its own short plan.
- **`LoginScreen.tsx` "Sign in to dux"** and **`App.tsx` `UnreachableScreen` `alt="dux"`** are NOT updated. Primary reason: a parallel agent is removing auth and owns these files (this plan must not touch them). Secondary, structural reason for the unreachable screen: it shows precisely when the server is unreachable, so bootstrap — and therefore `config.server.title` — has not been fetched and the configured name is unknowable at that point; the static fallback is the only value available. If either screen survives auth removal, updating it (the login card) or reading a last-seen title from `localStorage` (the unreachable screen) is a follow-up.
- **Pre-JS tab-title flash.** `index.html` ships a static `<title>dux</title>`; on a renamed instance the tab briefly shows `dux` before bootstrap resolves and `applyBootstrap` sets the real title. This is accepted. Eliminating it would require runtime HTML templating (giving up the fully-static `rust-embed` of `index.html`), which is not worth it.
- **Round-trip test starts from an empty config (low).** The Task 1 plain-write round-trip test writes to a fresh empty file, so it does not reproduce the exact "user's existing `[server]` block lacks `title`, then a plain save drops it" scenario the `apply_patches` lesson warns about. The test still fails if the `patch_table_str` call is omitted (the mutated `"dux #1"` value would not survive), so it is not blind; reproducing the pre-existing-file path is a marginal hardening, deferred.

## Self-Review (performed against the spec)

- **Spec coverage:** "title tag" → Task 4 (`document.title`). "projects pane title next to the dux logo, version below" → Task 5 desktop (Sidebar `AppSidebar`) + mobile (MobileShell `HomeScreen`), version/subtitle preserved. "configurable" → Tasks 1-2 (config field + bootstrap projection). Every requirement maps to a task.
- **Placeholder scan:** every code step shows real code; no TODO/TBD. The MobileShell `bootstrap` destructure is now an explicit required edit (Task 5 Step 2), not a parenthetical check.
- **Type/name consistency:** `resolveInstanceTitle` / `DEFAULT_INSTANCE_TITLE` (Task 3) are used verbatim in Tasks 4-5; `Bootstrap.title` (Task 3) matches `BootstrapView.title` JSON (Task 2) matches `config.server.title` (Task 1). `title` is `String` on the Rust side (always emitted) and `title?: string` on the TS side (back-compat with older servers / existing test fixtures that omit it).
- **Verified-against-real-code corrections (from adversarial review):** tests use the existing `loadStore()` helper, never the non-exported `bootAuth()`; the `document` stub is scoped to a nested `describe` so the `typeof document` guard stays exercised-as-absent elsewhere; the mobile wordmark lives in `HomeScreen`, which needs `bootstrap` added to its `useDux()`; decorative-logo `alt` attributes are left untouched (scope discipline); `resolveInstanceTitle` collapses internal newlines so the tab and wordmark never disagree; the `bootstrapApi.ts` "every field is required" comment is corrected when the optional `title?` is added.
