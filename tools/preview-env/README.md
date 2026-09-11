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

Two loopback ports are published: the web preview on `DUX_PORT` and the
concurrent-TUI journey's background server on `DUX_TUI_PORT`, which defaults to
`DUX_PORT + 208` (so 8790 pairs with 8998). Moving one moves both, which is what
lets a second stack come up beside a running one:

```bash
DUX_PORT=9000 ./up.sh         # web 9000, TUI-journey 9208
```

`shot.sh` reads `DUX_PORT` independently, so export it (or pass it to every
script in the session) when you move the web port, or the screenshots go to the
other stack.

### Iterate on code

```bash
./up.sh --restart             # rebuild the binary (incremental) + restart
```

A rebuilt binary is a new inode, so the container is recreated to pick it up.
Bringing it up waits until dux actually answers, rather than until the container
has started: a container that is up is a dux still opening its database and
sweeping its agents, and a seed or a page load aimed at that window is a race
nobody can see afterwards.

Recreating it from a shell that says nothing about the screenshot switches would
turn a screenshot container back into an ordinary preview (no stand-in `gh`, no
transcript provider, no `opencode` alias, and `--no-tailscale` back on), so what
the running container was brought up with carries forward and only an explicit
`DUX_SCREENS=` or `DUX_NO_TAILSCALE=` changes it. What does NOT survive a
restart is the agents: dux brings tabs back dormant by design, so the workspace
the scenes are written against is gone until something relights it. `reshoot.sh`
seeds before it shoots, which does exactly that; a scene driven by hand against
a just-restarted stack is shot against dormant agents, and its guards say so.

## Screenshots (host side)

`shot.sh` finds a Chromium automatically (`CHROME=` overrides: a cached
Playwright build, then common system binaries).

```bash
./shot.sh / home.png                  # one page, one PNG
./shot.sh / home-mobile.png --mobile  # phone viewport
./shot.sh '/#/agent/<sid>' agent.png  # a deep-linked position
```

Captures render at 2×: the desktop preset is 1440×900 CSS and writes
2880×1800, the phone preset 390×844 CSS and writes 780×1688.

Those are the conventions of the committed set under `website/public/screens/`,
and a reshoot that departs from them lands beside its neighbours looking wrong:

| | value |
| --- | --- |
| Scale | 2×, always (`--force-device-scale-factor=2`) |
| Desktop viewport | 1440×900 CSS (2880×1800 pixels) |
| Phone viewport | 390×844 CSS (780×1688 pixels), `--mobile` |
| TUI theme | `--theme dux_dark` (the script's own default is different) |

Element crops are cut from a capture at those viewports rather than shot at a
viewport of their own, so a crop's pixel size is whatever its element measures
at 2×.

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

**To regenerate a screenshot the docs already use, do not write a throwaway:
see [Regenerating the docs screenshots](#regenerating-the-docs-screenshots).**

For an ad-hoc journey of your own (clicking through a flow before screenshotting
something the docs do not carry), write a throwaway `puppeteer-core` script for
the journey at hand and delete it after:
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
theme, and terminal size are controlled by the harness. The script's default
theme is `catppuccin-mocha`; the screenshots committed under
`website/public/screens/` are taken with `--theme dux_dark`, so pass that when
reshooting one or the new image lands on a different palette from its
neighbours.

`tui-journey.example.js` shows the journey contract. For a special capture,
copy it to a throwaway `*.tmp.js` file and use the supplied `createAgent`,
`createStandaloneAgent`, `palette`, `seedLooseWorktree`, `sendKeys`, `sendText`,
`setFixture`, `captureText`, `sleep`, and `waitFor` helpers, plus the optional
`config` export for a setting no dialog can reach and the optional `expectText`
export, which refuses the capture unless those strings are on the screen it
captured (a throwaway may leave it off and is told on stderr that nothing
checked what it captured). Throwaway scripts are ignored
by Git, matching the web screenshot workflow; the journeys behind the docs
screenshots are not throwaways and live under `screens/scenes`.

## Regenerating the docs screenshots

Every image under `website/public/screens/` has a committed journey beside it,
so a screenshot a UI change made stale is re-run rather than reinvented:

```bash
cd tools/preview-env/screens
./reshoot.sh                        # every screenshot
./reshoot.sh phone-hub pr-banner    # just these
./reshoot.sh --list                 # the scenes there are
```

The scenes live in `screens/scenes/`, **one file per PNG, named after the PNG it
produces**, so `grep -rn phone-hub screens/scenes` finds the journey behind any
image in the docs. A browser scene exports `{ file, viewport, shoot }` and is
driven through `puppeteer-core`; a terminal UI scene exports the journey function
`tui-shot.sh` runs, with its grid, theme and crop hung off it. `screens/lib.js`
holds the shared browser and REST plumbing, and `screens/seed.js` builds the one
workspace every browser scene is shot against: the project, the six agents and
their states, the tabs, the terminals, the folder agent, the changed files, the
remote and the pull request. The seed is idempotent, so running it against an
already-seeded container converges to the same scene.

`reshoot.sh` brings the preview up itself if it is not already serving with the
screenshot fixtures, and seeds it. Both are for the browser scenes only: a
terminal UI journey runs in a disposable container of its own, so reshooting one
of those touches neither the preview nor its workspace. Those fixtures are the
pieces an ordinary preview must not have, and they are installed only under
`DUX_SCREENS=1`:

| Fixture | Why |
| --- | --- |
| `screens/fixtures/gh` | A stand-in `gh` answering only dux's auth probe and its bounded `pr view`, so the pull-request chip, banner and from-PR dialog are shootable without a GitHub login. |
| `screens/fixtures/notes-agent` | A transcript provider installed as `claude`, so the folder agent's pane shows a scripted session rather than a streaming fixture. |
| An `opencode` alias | dux refuses a tab for a CLI that is not on PATH, and the tab strip shows one tab per provider, so `opencode` points at the fake provider for the length of the run. |

Separately, and for **every** preview rather than only a screenshot run, the
entrypoint makes git non-interactive (`credential.helper` emptied,
`GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=/bin/true`). Nothing in the container can
answer a credential prompt and the container has a tty, so a git operation
against an unreachable remote would otherwise sit on `Username for ...` forever
and wedge whatever dux was doing.
| `DUX_NO_TAILSCALE=0` | Drops `--no-tailscale`, which otherwise refuses a live mode change for the whole run and changes what the Preferences dialog says. |

A website test asserts the loop stays closed: every PNG has a scene, every scene
has a PNG, and every PNG is shown by a docs page.

### Every scene says what its picture must show

A journey that drove itself into the wrong state still produces a perfectly
valid PNG, and the tool used to report success for one: a pane stuck on
"Attaching…", a sidebar that came back blank and a menu that had closed itself
under a take-over card all shipped in one run. So a scene states its own
caption's promise immediately before the shutter:

- A browser scene calls the guards in `screens/lib.js` (`expectNoCover`,
  `expectPanePainted`, `expectRows`, `expectStateWord`, `expectMenuOpen`,
  `expectDialogOpen`, `expectBanner`, `expectVisible`, `expectVisibleText`,
  `expectFieldValue`). Each returns nothing when the page holds up and throws
  one sentence naming the scene and what was missing when it does not.
  `expectPanePainted` is a measurement rather than a look at the socket: it
  screenshots the terminal and reads what fraction of its pixels differ from its
  own background.
- A terminal UI scene declares `expectText`, the strings that must appear in the
  captured cells. The driver refuses the capture when one is missing and prints
  the grid it got instead.

A refused scene writes no PNG, so the committed picture is left exactly as it
was; `reshoot.sh` says "refused" against it in the table and exits non-zero. The
website's loop test refuses a browser scene with no guard call and a terminal UI
scene with no `expectText`, so a new scene cannot ship unguarded.

A few things in these captures are the clock's or a random generator's and move
on every reshoot: the pet name in `tui-name-new-agent.png`, the run timestamp in
`tui-startup-command-log.png`, and how far a streaming fixture has counted in any
shot of a working agent. Everything else should come back the same.

## Logs / teardown

```bash
docker compose logs -f dux     # from tools/preview-env
docker compose down            # stop, keep volumes
docker compose down -v         # stop + wipe all preview state
```

These need no environment set: every variable in `compose.yml` defaults to what
the scripts pass, so plain compose commands work from this directory. The one
exception is a moved port: the `DUX_PORT + 208` derivation lives in `up.sh`, so
a compose command you run yourself with `DUX_PORT` set needs `DUX_TUI_PORT` set
too, or the second published port stays on its 8998 default.

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
| `screens/reshoot.sh` | Host: regenerate the docs screenshots under `website/public/screens`. |
| `screens/seed.js` | The one idempotent workspace every browser scene is shot against. |
| `screens/scenes/` | One committed journey per docs screenshot, named after the PNG. |
| `screens/fixtures/` | The stand-in `gh` and transcript provider a screenshot run installs. |
