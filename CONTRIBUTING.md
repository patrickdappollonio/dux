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
- **Server mode has no web UI to serve, if `crates/dux-web/web/dist` is empty.**
  The binary embeds a notice page instead, and:
  - every page served says the web UI was not built into this binary and how to
    rebuild it, so nobody stares at a blank screen;
  - the `dux server` startup banner carries a warning row saying the same thing;
  - `dux.log` gets a matching WARN line (this is the only one of the three the
    TUI-to-server flip reaches, since the flip keeps its themed status screen and
    must not print to stdout).
- **An existing `dist` is left alone, and the binary says so.** If you already
  have a real build in `crates/dux-web/web/dist`, setting the hatch does not
  delete it; the build script embeds it exactly as it is. Because that binary
  serves a real single-page app with real hashed assets, **nothing about using it
  reveals that the UI could be arbitrarily old**, so it is marked too: the startup
  banner and `dux.log` both carry a warning saying the web UI was not built from
  this source. The wording differs from the notice-page one on purpose, since here
  there IS a web UI. Unset the variable to rebuild.
- **The static-serving tests skip, in both cases.**
  `crates/dux-web/tests/static_serving.rs` cannot assert anything about a build
  that never happened, so the tests that need a real build print a `SKIPPED` line
  and return, with a reason naming which of the two cases you are in. Run
  `cargo test -- --nocapture` to see it. That covers the reused `dist` as well:
  those tests would otherwise pass against a UI of unknown age and tell you
  nothing about the code you are changing. The few tests that are about ROUTING
  rather than about the build deliberately do NOT skip; they read the same state
  to decide whether to expect the notice page or a real single-page-app shell, so
  they hold in both configurations.
- **The workflows refuse to run.** Because a skipped test is itself a place for a
  defect to hide, every job in the release and pull-request workflows depends on a
  guard job that fails when `DUX_DISABLE_UI_BUILD` is set, or when the
  `DUX_UI_BUILD_STATE` marker the build script stamps is set from outside. Be
  aware of what that guard can and cannot see: it inspects its own job's
  environment, so it catches a workflow-level `env:`, but not a job-level `env:`
  or a committed `.cargo/config.toml` that injects the variable into build
  scripts. A GitHub repository *variable* is neither caught nor a hazard: it is
  not exported into the job environment at all and is reachable only through the
  `vars` context. The gate that inspects the ARTIFACT rather than the
  configuration is `.github/scripts/smoke_archive.sh`, which greps the built
  binary for embedded content-hashed asset names before any release archive is
  published. It runs in the **release** workflow only, so on a pull request there
  is no artifact gate; the coverage there is the static-serving suite, which
  walks the whole bundle graph and applies the same floors on every PR.

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

## Writing a release body

**dux parses its own release notes and shows them to every user who updates.** On
the first launch after an upgrade it fetches the GitHub release for the tag it is
running and renders it as a "what's new" screen (the TUI modal and the web
`FirstLoadDialog`). The parser is `dux_core::release_notes::parse_release_body`,
and it is a two-level heading reader, **not** a Markdown renderer. Shape the body
wrong and the screen is wrong for everyone.

The format is short:

```markdown
## A headline that reads like a sentence

One or two paragraphs of intro prose.

### The first thing that changed

Whatever detail you like here.

### The second thing that changed

More detail.
```

Two generators add to whatever you write, and both **append**, after your own
sections: GitHub's "Generate release notes" button adds `## What's Changed` at the
end (look at the body of any past dux release), and the release workflow then adds
a `---` rule and a `## Installation` section after that. Nothing is ever prepended,
which is why your own `## ` line has to be the first thing in the body.

The rules, and what breaks if you skip one:

- **The body MUST begin with a single `## ` line.** That line becomes the screen's
  title. Skip it and the first `## ` in the file is GitHub's appended
  `## What's Changed`, so *that* becomes the headline, and any `### ` in the
  machine-written tail is merged into your feature list. Write nothing at all and
  the screen renders the commit list as one run-on paragraph.
- **Feature titles are `### ` lines.** They are rendered as a bulleted
  "In this release" list.
- **`# `, `#### `, and `##Title` (no space) are not headings to this parser.** They
  land in the intro prose with their `#` characters shown literally.
- **Only the prose before the first `### ` is shown.** The body text under each
  feature is deliberately dropped: the screen shows titles and links to the full
  notes for the rest.
- **The parse stops at the SECOND `## ` line.** That is how both appended
  sections, `## What's Changed` and `## Installation`, are kept off the screen. Do
  not use `## ` for anything of your own.
- **Close every code fence.** An unterminated ` ``` ` swallows the rest of the
  body.
- **A body with no readable notes says so.** A headline and nothing else is the
  obvious case, but so are a body that is only the appended `---` rule, only an
  HTML comment or `<br>`, or only invisible characters such as a zero-width space
  or a byte-order mark. Both screens then say "This release published no notes we
  could read" and point at the full notes. That is handled, but it is not what you
  want for a release people are upgrading into.

A malformed body never panics. It can leave the screen with nothing to show, and
then the screen SAYS so rather than rendering an empty panel; the failure mode is a
screen that says less than it should, not a blank one.
`crates/dux-core/src/release_notes.rs` holds a test per shape above, so if you
change the parser those tests are the contract to read first. The web mirrors the
"is there a body" rule in `crates/dux-web/web/src/lib/releaseNotes.ts`, and a Rust
test reads that file back so the two surfaces cannot drift into disagreeing about
the same release.

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
