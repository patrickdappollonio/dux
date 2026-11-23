# Browser checks

A suite of scripted checks that drive the web UI in a real browser against the
preview container and say, one line per check, whether a behaviour still holds.
They are the checks that used to be re-scripted by hand after most web UI
changes: dialog shapes and focus, the discard question, the New agent create,
toasts on a phone, the Delete agent pill, the macro picker, the phone editor,
Preferences and the browser's Back, and one driver at a time.

What it is not:

- **Not a replacement for unit tests.** The web's vitest suite and the Rust
  tests are still where a behaviour is pinned first; a check here covers what
  only a real browser shows (layout, focus across real events, history, a second
  device, what goes down a socket).
- **Not the docs screenshots.** Those are `../screens/` and `reshoot.sh`: a scene
  makes a picture for the docs, a check makes a verdict and keeps no picture
  unless it failed.
- **Not the CI journeys.** `crates/dux-journeys` runs container journeys in CI.
  This suite runs locally only, against the preview you already have up.

## Running it

```bash
tools/preview-env/checks/run.sh                       # every check
tools/preview-env/checks/run.sh one-driver            # one check, by name
tools/preview-env/checks/run.sh form-discard macro-picker
tools/preview-env/checks/run.sh --list                # names and what each protects
```

It runs against the preview at `http://127.0.0.1:8790` (`DUX_PORT` moves it, as
for every script here). `run.sh` takes care of the rest:

- **The preview.** The checks are written against the screenshot scenes'
  workspace, so the preview must be the screenshot one (`DUX_SCREENS=1`: the
  stand-in `gh` and the `opencode` alias the seed needs). When nothing answers
  or the preview is an ordinary one, `run.sh` recreates it with
  `DUX_SCREENS=1 DUX_NO_TAILSCALE=0 ../up.sh --restart`, which rebuilds the
  binary incrementally and keeps the volumes. It never does a plain bring-up.
  It leaves the preview in screenshot mode; `DUX_SCREENS=0 ../up.sh --restart`
  turns it back into an ordinary one.
- **The workspace.** Before the first check, `run.js` asks whether the workspace
  already is the seeded one (the six seeded agents and no others, every tab
  running, the folder agent's folder present, both terminals up). If it is, the
  run starts at once; if not, it runs the scenes' seed (`../screens/seed.js`)
  without the pull-request step, which takes about half a minute. A workspace
  holding agents the seed did not make is refused with the way out: wipe it
  with `docker compose down -v` in `tools/preview-env` and run again.
- **The browser.** `CHROME=` picks it, as for `reshoot.sh`; otherwise the same
  discovery runs. One browser per kind of device is launched for the run and
  every device a check opens is a fresh context in it, with a temporary profile
  removed when the browser closes.

Each check leaves the workspace as the seed left it, so runs go back to back
with no clean-up. A run of every check takes about 80 seconds on the
development machine once the workspace is seeded.

## The checks

| Check | Device | What it protects |
| --- | --- | --- |
| `dialog-shapes-computer` | computer | A confirm (Detach agent), a form (Rename agent), a picker (the launcher's New agent) and a report (Agent info) each float as a card, open with focus on the safe button, the first field, the search field or Close, and close on Escape with focus back on what opened them. |
| `dialog-shapes-phone` | phone | The confirm is a small card with its two buttons side by side; the form, picker and report cover the screen with the close in the title row and any footer on the bottom edge; no text field is focused on open; the close control hands focus back to the opener. |
| `form-discard` | computer | The environment variables form with nothing changed has its Save disabled with a reason and closes on Escape at once; with an edit, Escape asks "Discard unsaved changes?" and Keep editing keeps the text and puts focus back in the field. |
| `editor-dialog-backdrop` | computer | A dialog opened inside the editor (New file) has its own backdrop and closes on a press outside it, leaving the editor open. |
| `focus-across-resize` | computer | With a form open, resizing 1440 → 700 → 1440 keeps focus inside the dialog, and a typed letter lands in its field. |
| `new-agent-create` | computer | With the create held, three Enters send exactly one `POST /api/v1/sessions`; an existing branch's name raises "Attach to existing branch?", and Cancel, Escape and the browser's Back each return to the form with the name kept. |
| `phone-toasts` | phone | A toast starts below the lowest top chrome on the hub, on an agent pane (header and action flap) and under a full-screen dialog's title row, and at the top of the screen in theater mode. |
| `delete-agent-pill` | computer | Delete agent shows the neutral pill for an agent that loses nothing and the warning pill (alert icon, the sentence about commits) for one with a commit on no other branch, from open through 4 seconds later, with the dialog's height unchanged. |
| `macro-picker` | computer, phone | Rows carry no per-row button, typing filters, Enter runs the highlighted macro (its bytes go out on the pane's socket on a computer, its text lands in the message box on a phone), and "Edit macros…" opens the Macros dialog. |
| `phone-editor-tree` | phone | With no file open the editor's tree is shown; opening a file hides it; the header's toggle shows it again; opening and closing the explorer's `⋯` menu leaves it shown. |
| `phone-preferences-back` | phone | From both `/` and `#/`, Back from an edited Preferences topic returns to the topic list, and a second Back sets the dialog aside with a "Restore" toast without leaving the app. |
| `one-driver` | two computers | With a second device (another user agent) having taken over an agent, a confirm open on the first device has its action disabled with a reason naming that device, and the agent menu's changing entries are disabled under a reason naming it. |

`run.sh --list` prints the same sentences from the checks themselves.

## Reading a failure

A failing check prints one line that says what it expected and what it found:

```
FAIL form-discard (2.0s): with nothing changed, "Rename agent" should have its Save disabled; it is enabled
     screenshot: failures/form-discard.png
```

A screenshot of every page the check had open when it failed is saved in
`checks/failures/` (named `<check>.png`, or `<check>-<n>-<device>.png` when it
had more than one device). That folder is ignored by git and emptied at the start
of every run. A line starting "the check itself broke" is the script failing
rather than the product (a stack trace follows it); a line naming a timeout says
what was being waited for and what was seen last.

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

- **The helpers.** `harness.js` is the check vocabulary: `device` (a computer at
  1440x900 with a mouse, or a phone at 390x844 with touch and a phone's screen;
  `userAgent` makes it a different device), `goto`, `drive` (waits until this
  device drives the pane, pressing Take over when the card is up), `press` and
  `hover` (a real mouse click or tap on an element named the way a screen reader
  names it, once it holds still; a toast lying over the control is closed
  first and noted under the check's line, anything else in the way fails),
  `dismissToasts`, `paneMenu`, `openEditor`, `back`,
  `waitForDialog`, `waitForDialogGone`, `waitForNoDialog`, `dialogs`/`topDialog`
  (every open dialog measured: name, box, buttons, title row, footer, fields,
  whether focus is inside), `focused`, `waitFor` (a condition in the page),
  `until` (a condition on this side), `frames`, `expect` and `fail`.
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
  later"). Measure after two animation frames, and read a box once it holds
  still. Keep a check to one behaviour and well under 15 seconds.
- **Independent.** A check starts from the seeded workspace, sets up what it
  needs and puts back anything it changed, through `ctx.after(fn)`, which runs
  after its pages close, pass or fail. Never leave an agent, a commit, a saved
  setting or a draft behind; a create that must not happen is held or refused
  with request interception, never let through. Use `drive` before anything
  that types into or changes an agent: the previous check's device may still be
  driving it for a moment.
- **Server statuses.** The server replays a status it finished in the last 30
  seconds to every page that opens. A check that raises one (a config reload,
  say) exports `raisesServerStatuses: true`, and every `goto` for the next 30
  seconds waits briefly for the replayed toast and closes it, so it cannot land
  on the next check's press.
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
- **Left out on purpose:** the New agent form's 30-second "has not answered"
  deadline, which is about to become a setting; a check can be added once it
  is one.
