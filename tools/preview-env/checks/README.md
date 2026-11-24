# Browser checks

A suite of scripted checks that drive the web UI in a real browser against the
preview container and say, one line per check, whether a behaviour still holds.
They are the checks that used to be re-scripted by hand after most web UI
changes: dialog shapes and focus, the discard question, the New agent create
and its wait, toasts on a phone, the Delete agent pill, the macro picker, the
phone editor, Preferences and the browser's Back, and one driver at a time.

What it is not:

- **Not a replacement for unit tests.** The web's vitest suite and the Rust
  tests are still where a behaviour is pinned first; a check here covers what
  only a real browser shows (layout, focus across real events, history, a second
  device, what goes down a socket).
- **Not the docs screenshots.** Those are `../screens/` and `reshoot.sh`: a scene
  makes a picture for the docs, a check makes a verdict and keeps no picture
  unless it failed.
- **Not the CI journeys.** `crates/dux-journeys` runs container journeys in CI.
  This suite runs locally only, against the local preview container.

## Running it

From the repository root:

```bash
tools/preview-env/checks/run.sh                       # every check
tools/preview-env/checks/run.sh one-driver            # one check, by name
tools/preview-env/checks/run.sh form-discard macro-picker
tools/preview-env/checks/run.sh --reseed              # put the workspace back to the seed first
tools/preview-env/checks/run.sh --list                # names and what each protects
```

It runs against the preview at `http://127.0.0.1:8790`. `DUX_PORT` moves it, as
for every script here; a second stack beside the first needs a compose project
of its own as well, so name both:
`COMPOSE_PROJECT_NAME=b DUX_PORT=9000 tools/preview-env/checks/run.sh`.

`run.sh` takes care of the rest:

- **The build.** The web UI is compiled into the dux binary, so a check says
  something about your working tree only when the preview runs a binary built
  from it. Every run therefore builds the release binary from this checkout
  first (`cargo build --release --bin dux`, a fraction of a second when nothing
  changed). When the build fails the run stops there with the build's own
  error, and no check runs against the binary already in the preview.
- **The preview.** The runner compares the binary the container runs with the
  one just built (by their SHA-256), and restarts the preview with
  `tools/preview-env/up.sh --restart` when they differ, when nothing answers, or
  when the preview is an ordinary one rather than the screenshot one
  (`DUX_SCREENS=1`: the stand-in `gh` and the `opencode` alias the seed needs).
  A restart keeps the volumes and takes about ten seconds; an up-to-date build
  and preview cost a second or two. Before the first check it prints the build
  it checks, for example
  `>> checking dux 0.1.0, binary sha256 48cd6f47abde, built from ceb4b448 with uncommitted changes`.
  It never does a plain bring-up and never pulls an image. It leaves the
  preview in screenshot mode;
  `DUX_SCREENS=0 DUX_NO_TAILSCALE=1 tools/preview-env/up.sh --restart` turns it
  back into an ordinary one.
- **The image, the first time.** The preview image (`dux-preview`) must already
  exist: `docker image ls dux-preview` lists it. When it does not, the runner
  stops and says so. Building it is a plain `tools/preview-env/up.sh`, which
  downloads a base image and its packages, so ask the project owner before
  running that.
- **Docker access.** The scripts run `docker` directly when this shell can, and
  fall back to `sg docker -c` by themselves when the user was added to the
  `docker` group without logging in again.
- **The workspace.** Before the first check, the runner asks whether the
  workspace is the seeded one and carries nothing a check left behind: the six
  seeded agents and no others, every tab running, `refactor-cache` with its
  three tabs and every other agent its one, no terminal docked under an agent,
  the folder agent's folder present, both terminals up, no agent
  with a commit of its own, no branch named `checks-…` in the project, and no
  copy of the config file a check set aside. If it is, the run starts at once.
  If not, it says why, puts back what a check can leave (it deletes the extra
  agents with their working copies, closes their extra tabs and docked terminals, the `checks-…` branches and the agents' own
  commits, and restores the config file) and runs the scenes' seed
  (`../screens/seed.js`) without the pull-request step, which takes about half
  a minute. `--reseed` does all of that whatever the workspace looks like.
- **The browser.** `CHROME=` picks it, as for `reshoot.sh`; otherwise the same
  discovery runs. One browser per kind of device is launched for the run and
  every device a check opens is a fresh context in it, with a temporary profile
  removed when the browser closes.
- **Stopping it.** Ctrl-C (or a SIGTERM, or a crash of the runner) closes the
  check's pages, runs its clean-up and closes the browsers, which removes their
  temporary profiles, before leaving. A second Ctrl-C leaves at once; whatever
  that leaves in the workspace is put back at the start of the next run. A
  runner killed outright (SIGKILL) can do none of this: the workspace is still
  repaired by the next run, but its headless browsers keep running, with their
  profiles in `/tmp/puppeteer_dev_chrome_profile-*`, until you stop them.

Each check puts back what it changed, so runs go back to back with no clean-up
(`new-agent-wait`, for one, puts the config file back byte for byte, pass or
fail). Between checks the runner waits until the server has let go of every
pane the check's devices drove, so the next check never meets its take-over
card. A run of every check takes about 100 seconds on the development machine
once the workspace is seeded.

## The checks

| Check | Device | What it protects |
| --- | --- | --- |
| `dialog-shapes-computer` | computer | On a computer a confirm, a form, a picker and a report each float as a card, focus the safe button, the first field, the search field or Close on open, and close on Escape with focus back on what opened them. |
| `dialog-shapes-phone` | phone | On a phone a confirm is a small card with its two buttons side by side; a form, a picker and a report cover the screen with the close in the title row, a footer on the bottom edge and no text field focused; the close control hands focus back to what opened them. |
| `form-discard` | computer | A form with nothing changed has its Save disabled with a reason and closes on Escape at once; with an edit, Escape asks "Discard unsaved changes?" and Keep editing keeps the text and puts focus back in the field. |
| `editor-dialog-backdrop` | computer | On a computer a dialog opened inside the editor (New file) has its own backdrop and closes on a press outside it, leaving the editor open. |
| `focus-across-resize` | computer | With a form open, resizing the window 1440 → 700 → 1440 keeps focus inside the dialog, and a typed letter lands in its field. |
| `new-agent-create` | computer | New agent: with the create held, three Enters send exactly one POST /api/v1/sessions; an existing branch's name raises "Attach to existing branch?", and Cancel, Escape and the browser's Back each return to the form with the name kept. |
| `new-agent-wait` | computer | New agent: after a config reload sets server.new_agent_request_timeout_seconds to 3 on an open page, a held create frees the form after 3 seconds with the name kept and a warning naming 3 seconds, and a late 502 replaces that warning with one saying it is not known whether the agent was created. |
| `phone-toasts` | phone | On a phone a toast starts below the lowest top chrome on the hub, on an agent pane (header and action flap) and under a full-screen dialog's title row, goes back below the flap once that dialog closes, and sits at the top of the screen in theater mode. |
| `delete-agent-pill` | computer | Delete agent shows a pill saying nothing would be lost for an agent whose commits are all on another branch, and one saying its commit would be lost for an agent with a commit on no other branch, from open through 4 seconds later, without the dialog's height changing. |
| `macro-picker` | computer, phone | The macro picker's rows carry no per-row button, typing filters them, Enter runs the highlighted macro (its bytes go out on the pane's socket on a computer, its text lands in the message box on a phone), and "Edit macros…" opens the Macros dialog. |
| `phone-editor-tree` | phone | On a phone the editor shows the file tree with no file open, hides it when a file is opened, shows it again from the header's toggle, and keeps it shown after the explorer's ⋯ menu opens and closes, on a tap outside it and on Escape. |
| `phone-preferences-back` | phone | On a phone, from both "/" and "#/", the browser's Back from an edited Preferences topic returns to the topic list, and a second Back sets the dialog aside with a "Restore" toast without leaving the app. |
| `computer-dock-appears` | computer | On a computer, moving from an agent with no terminal to one whose terminal was last left folded away or alone on screen, and opening the first terminal on the agent on screen, each show the agent with its dock and no error card. |
| `one-driver` | two computers | With a second device having taken over an agent, a confirm that was open on the first device has its action disabled with a reason naming that device, and the agent menu's changing entries are disabled under a reason naming it. |
| `phone-tree-card` | phone, computer | On a phone a long press on a file in the editor's tree opens its menu as a card over the bottom of the screen that its own opening touch chooses nothing from, a tap outside it and the browser's Back close it without moving the page, a tap on an item chooses it, and while another device drives the agent the card says why its writes are greyed. |

The last column is each check's own `protects` sentence, word for word:
`run.sh --list` prints those sentences, and the runner (and `--list`) refuses
to start when the table and the checks disagree, naming every difference.

## Reading a failure

A failing check prints one line that says what it expected and what it found:

```
FAIL form-discard (2.0s): with nothing changed, "Environment: polish-onboarding" should have its Save disabled; it is enabled
     screenshot: failures/form-discard.png
```

A screenshot of every page the check had open when it failed is saved in
`checks/failures/` (named `<check>.png`, or `<check>-<n>-<device>.png` when it
had more than one device). That folder is ignored by git and emptied at the start
of every run. A line of the form `FAIL <check> (…): the check itself broke: …` is
the script failing rather than the product (a stack trace follows it); a line
naming a timeout says what was being waited for and what was seen last.

The run ends with a count and exits non-zero when anything failed.

## Adding a check

- **One file per check** in `cases/`, named for the behaviour in kebab case
  (`<surface>-<behaviour>`, e.g. `phone-toasts`). The runner picks up every
  `cases/*.js`. The file exports:

  ```js
  module.exports = {
    protects: "One sentence: the behaviour, as a person sees it.",
    async run(ctx, H) {
      const dev = await H.device(ctx, "computer") // or "phone"
      await H.goto(dev, H.agentHash(await H.agentId("polish-onboarding")))
      await H.drive(dev)
      // ...act, then H.expect(condition, "expected X; found Y")
    },
  }
  ```

  Give it a row in the table above too, with its `protects` sentence copied
  exactly; the runner will not start until you do.

- **The helpers.** `harness.js` is the check vocabulary: `device` (a computer at
  1440x900 with a mouse, or a phone at 390x844 with touch and a phone's screen;
  `userAgent` makes it a different device), `goto` (a fresh load, which also
  lets in and closes the toasts the server replays to a page that opens),
  `drive` (waits until this device drives the pane on screen; the take-over
  card there fails the check), `takeOver` (for a check that means to take a
  pane from another of its devices: waits for the card and presses Take over),
  `press` and `hover` (a real mouse click or tap, or a pointer move, on an
  element named the way a screen reader names it, once it has held still; one
  that never holds still, or anything lying over it, a toast included, fails
  the check), `reasonFor` (the reason a disabled control in a dialog gives when
  pointed at, read from its own tooltip), `dismissOwnToasts` (closes toasts the
  check raised itself, and says which), `liveToasts`, `paneMenu`, `openEditor`,
  `back`, `waitForDialog`, `waitForDialogGone`, `waitForNoDialog`,
  `dialogs`/`topDialog` (every open dialog measured: name, box, buttons, title
  row, footer, fields, whether focus is inside), `focused`, `focusIsMarked`
  (whether focus is on the very element a press remembered with `mark`, not
  merely one with the same name), `waitFor` (a condition in the page), `until`
  (a condition on this side), `frames`, `expect` and `fail`.
  `dialogs.js` holds the representative dialog of each kind and how to open it.
  The browser, the REST client (`H.lib.api`, `H.lib.agents`), the container
  shell (`H.lib.containerSh`) and the seed are the screenshot scenes' own, in
  `../screens/lib.js` and `../screens/seed.js`: extend those when a check needs
  something new there, so the scenes and the checks share one driver.
- **Assert behaviour, never a class name.** Find things by role and accessible
  name, and assert what a person gets: which dialog is open, where its box sits,
  what has focus, what a field holds, what request went out, what a socket sent.
  A structural attribute the app sets on purpose (`role`, `data-slot` on the
  modal's parts) is fine to find a part by; styling classes are not, because
  they change for reasons that have nothing to do with the behaviour.
- **Fail in words a person can act on.** Every `expect` says what was expected
  and what was found ("should open with focus on its Cancel button; focus is on
  input "Agent name""). Quote names and measurements; never just "assertion
  failed".
- **Fast and deterministic.** No fixed sleeps where a condition can be waited
  for: `waitFor`/`until` with a timeout and a message. A fixed wait is allowed
  only when the time is itself the behaviour (the Delete agent pill's "4 seconds
  later", three Enters at a person's pace), or when the check waits for
  something NOT to happen (a second create that must not be sent), with a
  comment saying so. Measure after two animation frames, and read a box once it
  holds still. Keep a check to one behaviour and well under 15 seconds.
- **Independent.** A check starts from the seeded workspace, sets up what it
  needs and puts back exactly what it changed, through `ctx.after(fn)`, which
  runs after its pages close, pass or fail, and on Ctrl-C. Register the undo
  before making the change, so an interrupt between the two still runs it.
  Never leave an agent, a commit, a branch, a saved setting or a draft behind; a
  create that must not happen is held or refused with request interception,
  never let through. A second Ctrl-C or a killed runner skips the undo, so a
  check that can change something new also extends the start-of-run check
  (`workspaceProblem`) and its repair (`backToSeed`) in `run.js` in the same
  change: a branch a check makes is named `checks-…`, and a file it edits is
  copied aside first, as `new-agent-wait` does with the config file.
- **Driving.** Use `drive` before anything that types into or changes an agent.
  A check that opens a second device on a pane its first device drives says so
  with `takeOver`; the take-over card anywhere else is a failure.
- **Toasts.** The server replays every status it finished in the last 30
  seconds to each page that opens. `goto` waits for that replay to end and
  closes those toasts, since they belong to earlier checks. After that, a
  toast lying over a control a check presses fails the check, naming the toast
  and the control: covering a control is exactly the kind of bug a check is
  for. A check that raised a toast itself and has to reach past it closes it
  with `dismissOwnToasts(dev, "what it raised")`.
- **Prove it fails.** Before committing a new check, run it once against a state
  where the behaviour does not hold (a different dialog, a skipped setup step, a
  wrong expectation) and see its FAIL line read sensibly.

## Keeping it in step

**A change to a screen or behaviour a check covers updates that check in the same
change, a new dialog or recurring browser-verified behaviour gets a check, and a
check that no longer matches the product is fixed or deleted, never skipped.**

There is no skip mechanism on purpose: a check that is wrong is either made right
or removed, with the reason in the commit message. Run the whole suite after any
web UI change, and before committing one.

## Known limits

- **Headless Brave (and Chromium) keeps user activation forever.** A real
  browser drops it, and the app pushes a dialog's history entry only with one,
  so a check about history passes here in cases where a person's browser could
  behave differently. Paths that depend on having no activation cannot be
  checked here.
- **Local only.** It is not in CI yet; it needs the preview container and a
  Chromium on the host.
- **One workspace.** It shares the preview with the screenshot scenes, so do not
  run it while `reshoot.sh` is running.
