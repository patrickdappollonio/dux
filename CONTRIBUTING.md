# Contributing to dux

Thanks for looking. This file covers building dux from source and the checks a
pull request has to pass. For how the app is *designed*, read `CLAUDE.md` at the
repo root: it holds the design tenets, and if a change conflicts with a tenet,
the tenet wins.

## What you need

- **Rust** (stable). The workspace tracks whatever `dtolnay/rust-toolchain@stable`
  installs in CI, so a current stable toolchain is the right target.
- **Node 22 or newer** plus npm. The web UI is a React app that is **compiled into
  the `dux` binary**, so a normal `cargo build` builds it. If you have no Node
  toolchain, see [Building without the web UI](#building-without-the-web-ui).
- **`git`**, which dux is built around.
- **`gh`** (optional) for the GitHub integration.

Target platforms are macOS and Linux. Windows users run dux under WSL2, which is
Linux; there are no Windows code paths.

## Building

```bash
cargo build
```

That is all. `crates/dux-web/build.rs` runs `npm ci` (when the lockfile is newer
than `node_modules`) and then `npm run build` in `crates/dux-web/web`, gzips the
result in place, and `rust_embed` bakes it into the binary.

**A failed frontend build fails the Rust build.** It used to print a
`cargo:warning`, embed a placeholder page and succeed, which meant a release could
ship binaries with no web UI and every check green. If you see the build abort with
a tsc or Vite error, that is working as intended. To see the underlying error:

```bash
cargo build -vv -p dux-web
# or reproduce it directly
cd crates/dux-web/web && npm ci && npm run build
```

## Building without the web UI

Set `DUX_DISABLE_UI_BUILD` to skip the frontend build entirely:

```bash
DUX_DISABLE_UI_BUILD=1 cargo build
```

**Any non-empty value counts as set.** `DUX_DISABLE_UI_BUILD=1`,
`DUX_DISABLE_UI_BUILD=true` and `DUX_DISABLE_UI_BUILD=please` all skip the build.
An empty value (`DUX_DISABLE_UI_BUILD=`) is treated as unset.

This is for contributors working only on Rust, and for machines with no Node
toolchain. What it does and does not do:

- **The terminal UI is completely unaffected.** `dux` with no arguments behaves
  exactly as it does in a normal build.
- **Server mode has no web UI to serve.** If `crates/dux-web/web/dist` is empty,
  the binary embeds a notice page instead, and:
  - every page served says the web UI was not built into this binary and how to
    rebuild it, so nobody stares at a blank screen;
  - the `dux server` startup banner carries a warning row saying the same thing;
  - `dux.log` gets a matching WARN line (this is the only one of the three the
    TUI-to-server flip reaches, since the flip keeps its themed status screen and
    must not print to stdout).
- **The static-serving tests skip.** `crates/dux-web/tests/static_serving.rs`
  cannot assert anything about a build that never happened, so the tests that need
  a real build print a `SKIPPED` line and return. Run
  `cargo test -- --nocapture` to see it. Because a skipped test is itself a place
  for a defect to hide, the release and pull-request workflows **refuse to run at
  all** with `DUX_DISABLE_UI_BUILD` set.
- **An existing `dist` is left alone.** If you already have a real build in
  `crates/dux-web/web/dist`, setting the hatch does not delete it; the build script
  embeds it as-is and warns that it may be stale. Unset the variable to rebuild.

Skipping the frontend build deliberately is supported. Letting a *failed* frontend
build through quietly is not, and that distinction is the whole point of the flag.

## Checks

From the repo root:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

From `crates/dux-web/web`:

```bash
npm run lint
npm run test
npm run build
```

`cargo clippy --all-targets --all-features -- -D warnings` is a CI gate and the PR
check runs that exact command, so run it locally before pushing. A new stable Rust
release can enable lints that previously passed; fix the code rather than
suppressing the lint unless there is a specific, documented reason.

Every change should come with unit tests, and integration tests where that is
feasible and low-lift. When you are fixing a bug, aim to prove the diagnosis with
a failing test before you fix it.

## Interactive testing

Ask for a smoke test rather than assuming one: `cargo run` starts the TUI, and
`cargo run -- server` starts the web server. Both bind the single-instance lock in
your dux config directory, so only one can run at a time against a given config.

## A few house rules worth knowing up front

- **Commit messages are plain sentences.** No `feat:`/`fix:`/`chore:` prefixes and
  no structured trailers.
- **All settings are configurable, and the config file is the documentation.** New
  options get inline comments explaining them.
- **The TUI styles through `crates/dux-tui/src/app/theme.rs`; never hardcode a
  color.** The web UI styles through the shadcn/base-ui token CSS variables.
- **Keybindings are user-configurable, so never hardcode a key label in
  user-facing text.** Look it up through the runtime bindings.
- **Keep docs in step.** `README.md`, the marketing site in `website/`, and the
  docs pages in `website/docs/` are part of the change, not a follow-up.

`CLAUDE.md` is the long form of all of the above and quite a lot more. It is worth
reading before a substantial change.
