# dux preview environment

An isolated, Docker-only way to run **dux web mode** so its UI can be driven
and screenshotted without touching the developer's real dux instance. Built so
a person or an agent can SEE UI work: bring it up, drive real user journeys,
capture PNGs.

> **Never run `dux server` (or the TUI) directly on a development host to
> inspect the UI, and never kill dux processes on the host.** A directly-run
> instance shares the developer's real `~/.config/dux` (config, sessions
> database, locks) with whatever dux they already have running, and on this
> project the developer is often driving the working session FROM dux itself:
> a stray instance corrupts state, and a killed process can destroy their live
> session. This container is the sanctioned path; Docker-only is a decision,
> not a limitation.

## What it is

- A Docker image with the **runtime** deps: recent glibc (rolling Arch base),
  git, node/npm, and the `claude` + `codex` CLIs installed via npm (they run
  and prompt for login, exactly as they would anywhere).
- The **dux binary is built on the host** and bind-mounted read-only. The host
  already has cargo plus the node setup `build.rs` needs to embed `web/dist`,
  and building on the same machine avoids glibc/CPU ABI skew.
- All dux state (config, sessions, seeded repos) lives in **named volumes**,
  so the container cannot reach the host's real dux config
  (`DUX_HOME=/data/dux`).
- A seeded config adds a **`fake` streaming provider** (`fake-agent.sh`): no
  real agent CLI needs to be authenticated (or even work) for UI testing. It
  streams forever, so working-state visuals are exercisable; closing its tab
  returns the agent to Idle.
- Two seeded git repos (`demo-api`, `demo-web`) so the project picker has
  targets.

## One-time host setup

Docker group access, once per account:

```bash
sudo usermod -aG docker $USER
```

`up.sh` uses `docker` directly when the shell already has access and falls
back to `sg docker` when the group was added without a re-login.

## Bring it up

```bash
cd tools/preview-env
./up.sh                       # builds dux (this repo) + starts the container
DUX_SRC=/path/to/worktree ./up.sh   # preview a different branch/worktree
```

Then open `http://127.0.0.1:8790` (loopback only; `DUX_PORT` overrides).

### Iterate on code

```bash
./up.sh --restart             # rebuild the binary (incremental) + restart
```

A rebuilt binary is a new inode, so the container is recreated to pick it up.

## Screenshots (host side)

`shot.sh` finds a Chromium automatically (`CHROME=` overrides: a cached
Playwright build, then common system binaries).

```bash
./shot.sh / home.png                  # one page, one PNG
./shot.sh / home-mobile.png --mobile  # phone viewport
./shot.sh '/#/agent/<sid>' agent.png  # a deep-linked position
```

Captures render at 2×: the desktop preset writes 2560×1800 and the phone preset
780×1688.

The scale is asked of the browser (`--force-device-scale-factor=2`) rather than
of the viewport, and SwiftShader is requested by name. Both matter for the
terminal: it paints through xterm's webgl renderer, and a webgl canvas captured
under an emulated device scale factor above 1 comes back **black** in headless
Chromium, while the old `--disable-gpu` quietly left every capture on the DOM
renderer instead. If a canvas still comes back black on some host, capture the
whole page (`fullPage: true`, or a `clip` with `captureBeyondViewport`) and crop
the PNG afterwards; that path renders the canvas rather than reading back the
compositor's surface.

A capture opens a real connection to the PTY, so it can arrive as a watcher and
come back with the full-pane take-over card over the terminal instead of the
terminal itself. When that happens, press **Take over** in a journey script
before capturing.

For interactions (clicking through a flow before screenshotting), write a
throwaway `puppeteer-core` script for the journey at hand and delete it after:
`shot.js` shows the connection boilerplate (launch args, viewport, the
forwarded port), and `puppeteer-core` is already in this directory's
`package.json`. Use journey-specific selectors instead of a generic action DSL;
real flows need bespoke selectors, and the extra abstraction cannot express
many UI interactions. Driving the app over REST (`/api/v1/...`) is often faster
than clicking; see the routes in `crates/dux-web/src/`.

## TUI screenshots

`tui-shot.sh` runs the real terminal UI in a disposable Docker container at a
fixed terminal grid. It drives a small journey script, captures the styled terminal
cells, and renders them in headless Chromium. The capture has its own `DUX_HOME`,
repositories, and process lock, so it can run while the web preview is up.

The default 160×45 grid renders 2632 pixels wide at 2× scale and is the desktop
baseline. Use `--cols` and `--rows` only for a different terminal class.

```bash
./tui-shot.sh tui-journey.example.js shots/tui-workspace.png
./tui-shot.sh palette.tmp.js shots/tui-palette.png --cols 160 --rows 45
./tui-shot.sh narrow.tmp.js shots/tui-narrow.png --cols 100 --rows 30
./tui-shot.sh sidebar.tmp.js shots/tui-sidebar.png --crop sidebar
```

The face is **MonoLisa Nerd Font Mono** when the host has it installed, and the
bundled Dux Mono stack otherwise. MonoLisa is commercial, so it is referenced by
family name and never vendored here; a host without it still captures correctly
and says so on stderr, at a slightly different cell size than the committed
screenshots. The font size is 12.5 for a measured reason spelled out in
`tui-shot.js`: xterm's DOM renderer places cells at the font's unrounded
advance, and any size whose advance misses a whole device pixel turns every
`▄`/`▀`/`█` row into a comb of antialiased seams. 12.5 is the only readable size
that lands exactly for both faces.

`--crop sidebar` frames the left pane instead of the whole screen. The rect is
read out of the capture's own text grid (the pane's border columns, its top
border row, and every row down to the last one with content in it plus a row of
air) and multiplied by the cell metrics measured off the live terminal, so a
crop edge always lands on a cell boundary rather than slicing a border column
down the middle. Crops are flush to their cells and never exceed 1.5:1.

Each run writes four artifacts beside the requested PNG: the image, styled ANSI
cells, a plain-text grid, and JSON capture details. The example journey does not use
the network. Their repositories, commit dates, project names, provider output,
theme, and terminal size are controlled by the harness.

`tui-journey.example.js` shows the journey contract. For a special capture,
copy it to a throwaway `*.tmp.js` file and use the supplied `createAgent`,
`sendKeys`, `sendText`, `captureText`, `sleep`, and `waitFor` helpers. Throwaway
scripts are ignored by Git, matching the web screenshot workflow.

## Logs / teardown

```bash
docker compose logs -f dux     # from tools/preview-env
docker compose down            # stop, keep volumes
docker compose down -v         # stop + wipe all preview state
```

These need no environment set: every variable in `compose.yml` defaults to what
the scripts pass, so plain compose commands work from this directory.

## Login-walled providers

`claude` and `codex` run but require login on first spawn; that login screen
is itself a valid state to screenshot. For everything else use the `fake`
provider: UI testing here never requires authenticating a real agent.

## Platform: Linux x86_64 host vs macOS ARM

The default "build on host, mount the binary" path is **Linux-only** (the
container can only run a Linux binary of its own architecture); `up.sh`
refuses on macOS rather than mount garbage.

- **Linux host:** default path. Fast incremental cargo, zero ABI skew.
- **macOS ARM host:** build dux **in-container** for linux/arm64 instead: add
  rust to the Dockerfile, mount the source, and build with cached target +
  cargo-registry volumes. First build is minutes; incrementals fast.

## Glibc note

The mounted host binary needs container glibc >= host glibc.
`archlinux:latest` (rolling) normally satisfies this. If `dux --help` inside
the container reports `GLIBC_2.xx not found`, pass a base image matching your
host (`BASE_IMAGE=<image> ./up.sh`) or use the in-container build.

## Files

| File | Role |
| --- | --- |
| `Dockerfile` | Runtime image (glibc/git/node/tmux + claude/codex CLIs). |
| `entrypoint.sh` | Seeds config + demo repos, then serves the web UI. |
| `fake-agent.sh` | Fake provider with live preview output and deterministic capture fixtures. |
| `compose.yml` | Defines the isolated web preview and opt-in TUI capture service. |
| `up.sh` | Host: build binary + start/restart the container. |
| `shot.sh` / `shot.js` | Host: screenshot one page of the running preview; also the boilerplate reference for throwaway interaction scripts. |
| `tui-shot.sh` / `tui-shot.js` | Host: run a deterministic TUI scene and render its captured cells as a PNG (`--crop sidebar` frames the left pane). |
| `tui-driver.js` | Container: seed disposable state, run one journey, and export capture artifacts. |
| `tui-journey.example.js` | Minimal example that agents copy into ignored, task-specific journeys. |
