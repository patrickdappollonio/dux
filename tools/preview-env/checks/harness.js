// What every check is written with: devices, waits, presses and the questions a
// check asks of a dialog. The browser, the REST client and the seeded workspace
// are the screenshot scenes' own (../screens/lib.js); nothing here repeats them.
const lib = require("../screens/lib.js")

const { BASE, sleep } = lib

// A failed expectation. The message is the whole report a person gets, so it
// says what the check expected and what it found.
class CheckFailure extends Error {}

function fail(message) {
  throw new CheckFailure(message)
}

function expect(ok, message) {
  if (!ok) fail(message)
}

// --- Devices ---------------------------------------------------------------

// One browser per kind of device for the whole run, and a fresh context (its
// own storage, so its own browser id and nothing remembered) for every device a
// check asks for. Launching a browser costs seconds; a context costs nothing.
const browsers = {}

async function browserFor(kind) {
  if (!browsers[kind]) {
    browsers[kind] = lib.launch({
      mobile: kind === "phone",
      scale: 1,
      finePointer: kind === "computer",
    })
  }
  return browsers[kind]
}

async function closeBrowsers() {
  for (const kind of Object.keys(browsers)) {
    const browser = await browsers[kind].catch(() => null)
    delete browsers[kind]
    if (browser) await browser.close().catch(() => {})
  }
}

// The user agent a second device is given, so the server names it as something
// other than the first one ("Firefox on Windows").
const OTHER_DEVICE_UA =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0"

// kind is "computer" (1440x900, a mouse) or "phone" (390x844, touch, a phone's
// screen). Every device a check opens is closed by the runner after the check.
async function device(ctx, kind, { userAgent = null, width, height } = {}) {
  const browser = await browserFor(kind)
  const context = await browser.createBrowserContext()
  ctx.contexts.push(context)
  const phone = kind === "phone"
  const opened = await lib.devicePage(context, {
    mobile: phone,
    width,
    height,
    screen: phone,
    userAgent,
  })
  await opened.page.evaluateOnNewDocument(installNameOf)
  const dev = { kind, phone, context, log: ctx.log, ...opened }
  opened.page.on("pageerror", (error) => ctx.log(`page error on the ${kind}: ${error.message}`))
  ctx.devices.push(dev)
  return dev
}

// --- Waiting ---------------------------------------------------------------

// Polls `probe` (run in the page when it is a function of no node arguments
// handed to page.evaluate, or on this side when `local` is set) until it
// returns something truthy, and fails with `what` and the last value seen.
async function waitFor(page, probe, what, { timeout = 8000, gap = 100, arg } = {}) {
  const deadline = Date.now() + timeout
  let last
  for (;;) {
    last = await page.evaluate(probe, arg)
    if (last && (typeof last !== "object" || last.ok !== false)) return last
    if (Date.now() > deadline) break
    await sleep(gap)
  }
  const seen = last && typeof last === "object" && "seen" in last ? last.seen : JSON.stringify(last)
  fail(`timed out after ${timeout / 1000}s waiting for ${what}; last saw ${seen}`)
}

// The same, for a condition read on this side of the wire.
async function until(read, what, { timeout = 8000, gap = 100 } = {}) {
  const deadline = Date.now() + timeout
  let last
  for (;;) {
    last = await read()
    if (last) return last
    if (Date.now() > deadline) break
    await sleep(gap)
  }
  fail(`timed out after ${timeout / 1000}s waiting for ${what}`)
}

// Two animation frames: what a measurement waits for after anything moved.
async function frames(page, n = 2) {
  await page.evaluate(
    (count) =>
      new Promise((done) => {
        const step = (left) => (left === 0 ? done() : requestAnimationFrame(() => step(left - 1)))
        step(count)
      }),
    n,
  )
}

// --- Navigation -------------------------------------------------------------

const WORKSPACE_READY = () =>
  document.querySelector('[aria-label="Session actions"]') !== null ||
  document.querySelector('[data-testid="terminal-pane"]') !== null

// Loads an address and waits for the workspace to be on screen rather than for
// a fixed time: the app has painted once a sidebar row, a hub row or a pane is
// there (`ready` names something else for a screen with none of those). The
// no-password banner is dismissed the way the scenes dismiss it.
async function goto(dev, hash = "", { root = "/", ready = WORKSPACE_READY, what = "the workspace to load" } = {}) {
  // Through a blank page, so every goto is a fresh load: an address differing
  // from the current one only after "#" is otherwise a same-document jump that
  // leaves the last step's menus, history entries and focus where they were.
  await dev.page.goto("about:blank")
  await dev.page.goto(BASE + root + (hash || ""), { waitUntil: "domcontentloaded", timeout: 30000 })
  await waitFor(dev.page, ready, what, { timeout: 20000 })
  // The preview serves with no password, so the banner comes; it is waited for
  // briefly rather than assumed, since it renders once the config has loaded.
  await waitFor(dev.page, () => document.querySelector('[data-testid="no-auth-banner"]') !== null, "the banner", {
    timeout: 3000,
  }).catch(() => {})
  await lib.dismissNoPasswordBanner(dev.page)
  // The server replays a status it finished in the last 30 seconds to every page
  // that opens, so for that long after a check raised one, a new page gets its
  // toast a moment after loading, over whatever the next press aims at. It is
  // waited for and closed here, as a person would close it.
  if (Date.now() < replay.until) {
    const came = await waitFor(dev.page, () => document.querySelector("[data-sonner-toast]") !== null, "a replayed toast", {
      timeout: 1500,
    }).catch(() => false)
    if (came) await dismissToasts(dev)
  }
  await frames(dev.page)
}

// Until when a page that opens may be handed a replayed toast. The runner moves
// it on after a check that says it raises server statuses.
// It is kept in a file beside the failure pictures as well, because the run
// that raised the status may be the previous one, seconds ago.
const REPLAY_WINDOW_MS = 30000
const replayFile = require("path").join(__dirname, "failures", ".replay-until")
const replay = { until: readReplayUntil() }

function readReplayUntil() {
  try {
    return Number(require("fs").readFileSync(replayFile, "utf8")) || 0
  } catch {
    return 0
  }
}

function serverStatusRaised() {
  replay.until = Date.now() + REPLAY_WINDOW_MS
  try {
    require("fs").mkdirSync(require("path").dirname(replayFile), { recursive: true })
    require("fs").writeFileSync(replayFile, String(replay.until))
  } catch {
    // Only the next run's first half minute is less careful without it.
  }
}

// The agent's editor, opened over a pane this device drives: the editor's
// writes (a new file among them) are disabled on a row somebody else drives.
const EDITOR_READY = () =>
  [...document.querySelectorAll("button")].some((b) => /^(Hide|Show) the file explorer$/.test(b.getAttribute("aria-label") || ""))

async function openEditor(dev, sessionId) {
  await goto(dev, `#/agent/${sessionId}`)
  await drive(dev)
  await dev.page.evaluate((hash) => {
    location.hash = hash
  }, `#/agent/${sessionId}/editor`)
  await waitFor(dev.page, EDITOR_READY, "the editor to open", { timeout: 10000 })
  await frames(dev.page)
}

async function back(dev) {
  await dev.page.evaluate(() => history.back())
  await sleep(300)
  await frames(dev.page)
}

// --- Finding and pressing -----------------------------------------------------

// What a control is called, the way a screen reader would say it: its label,
// else what aria-labelledby points at, else its text, else (a field with none
// of those) its placeholder. Installed into every page
// before the app runs, so every probe below names things the same way.
function installNameOf() {
  window.__checkNameOf = (el) => {
    const by = (el.getAttribute("aria-labelledby") || "")
      .split(/\s+/)
      .filter(Boolean)
      .map((id) => (document.getElementById(id) || {}).textContent || "")
      .join(" ")
    const said = el.getAttribute("aria-label") || by || el.textContent || el.getAttribute("placeholder") || ""
    return said.replace(/\s+/g, " ").trim()
  }
}

// The centre of the first visible element matching `selector` whose name
// matches `name` (a regex source), inside `within` when given.
async function locate(page, { name = null, selector = "button,[role=menuitem]", within = null, nth = 0, last = false }) {
  return page.evaluate(
    (spec) => {
      const nameFn = window.__checkNameOf
      const root = spec.within ? [...document.querySelectorAll(spec.within)].at(-1) : document
      if (!root) return null
      const re = spec.name === null ? null : new RegExp(spec.name)
      const hits = [...root.querySelectorAll(spec.selector)].filter((el) => {
        const r = el.getBoundingClientRect()
        if (r.width < 1 || r.height < 1) return false
        const style = getComputedStyle(el)
        if (style.visibility === "hidden") return false
        return !re || re.test(nameFn(el))
      })
      const el = spec.last ? hits.at(-1) : hits[spec.nth]
      if (!el) return null
      el.scrollIntoView({ block: "nearest" })
      const r = el.getBoundingClientRect()
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
    },
    { name, selector, within, nth, last },
  )
}

// A real press, by mouse on a computer and by a tap on a phone, on the element
// a spec names. A press is what a person does; element.click() skips the
// pointer events base-ui menus act on.
async function press(dev, spec, { timeout = 5000 } = {}) {
  const s = typeof spec === "string" ? { name: spec } : spec
  let at = null
  const deadline = Date.now() + timeout
  while (!at && Date.now() < deadline) {
    at = await locate(dev.page, s)
    if (!at) await sleep(100)
  }
  if (!at) fail(`there is nothing to press named /${s.name}/ (${s.selector || "button,[role=menuitem]"})`)
  // A sheet or a menu slides in, and a screen still loading shifts its rows, so
  // the point is read again until it has held still for about 200ms: a tap on
  // where an item was a moment ago lands on something else (measured: the
  // phone editor's tree moved a file's row 16px after it was first drawn, and
  // the tap's click went to the row's container).
  let stillFor = 0
  for (let i = 0; i < 40 && stillFor < 3; i++) {
    await frames(dev.page)
    await sleep(40)
    const now = await locate(dev.page, s)
    if (!now) break
    stillFor = Math.abs(now.x - at.x) < 0.5 && Math.abs(now.y - at.y) < 0.5 ? stillFor + 1 : 0
    at = now
  }
  // A toast can come down over the control at any time (the server raises its
  // own). A person closes it and presses again, so the check does the same, and
  // says so under its result line; anything else in the way is a failure.
  for (let tries = 0; ; tries++) {
    const cover = await coveredBy(dev.page, at, s)
    if (!cover) break
    if (!cover.toast || tries >= 3) {
      fail(`the control named /${s.name}/ is covered by ${cover.what}, so a press would land there instead`)
    }
    dev.log(`closed a toast lying over /${s.name}/: "${cover.what}"`)
    await dismissToasts(dev)
    at = (await locate(dev.page, s)) || at
  }
  if (dev.phone) await dev.page.touchscreen.tap(at.x, at.y)
  else await dev.page.mouse.click(at.x, at.y)
  await frames(dev.page)
}

// What a press at a point would land on, when it is not the control a spec names.
async function coveredBy(page, at, spec) {
  return page.evaluate(
    ({ x, y }, sp) => {
      const hit = document.elementFromPoint(x, y)
      if (!hit) return null
      const re = sp.name === null || sp.name === undefined ? null : new RegExp(sp.name)
      const sel = sp.selector || "button,[role=menuitem]"
      for (let el = hit; el; el = el.parentElement) {
        if (el.matches && el.matches(sel) && (!re || re.test(window.__checkNameOf(el)))) return null
      }
      const toast = hit.closest("[data-sonner-toast]")
      if (toast) return { toast: true, what: toast.textContent.replace(/\s+/g, " ").trim().slice(0, 120) }
      // A control drawn inside a label or a wrapper of the named element still
      // reaches it; only a different control or a layer above it is in the way.
      const target = [...document.querySelectorAll(sel)].find(
        (el) => (!re || re.test(window.__checkNameOf(el))) && el.contains(hit),
      )
      if (target) return null
      return { toast: false, what: `${hit.tagName.toLowerCase()} "${(window.__checkNameOf(hit) || "").slice(0, 60)}"` }
    },
    at,
    { name: spec.name === undefined ? null : spec.name, selector: spec.selector },
  )
}

// Closes every toast on screen with its own close button, and waits for them to go.
async function dismissToasts(dev) {
  for (let i = 0; i < 6; i++) {
    const close = await dev.page.evaluate(() => {
      const b = [...document.querySelectorAll("[data-sonner-toast] button")].find(
        (x) => window.__checkNameOf(x) === "Close toast" && x.getBoundingClientRect().width > 0,
      )
      if (!b) return null
      const r = b.getBoundingClientRect()
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
    })
    if (!close) break
    if (dev.phone) await dev.page.touchscreen.tap(close.x, close.y)
    else await dev.page.mouse.click(close.x, close.y)
    await sleep(150)
  }
  await waitFor(dev.page, () => document.querySelector("[data-sonner-toast]") === null, "the toasts to leave", {
    timeout: 3000,
  })
  await frames(dev.page)
}

async function hover(dev, spec) {
  const s = typeof spec === "string" ? { name: spec } : spec
  const at = await locate(dev.page, s)
  if (!at) fail(`there is nothing to hover named /${s.name}/`)
  await dev.page.mouse.move(at.x, at.y)
  await frames(dev.page)
}

// What has focus, as a sentence a failure can quote.
async function focused(page) {
  return page.evaluate(() => {
    const nameFn = window.__checkNameOf
    const el = document.activeElement
    if (!el || el === document.body) return { what: "nothing (the page body)", typed: false, name: "" }
    const typed =
      el.tagName === "TEXTAREA" ||
      el.isContentEditable ||
      (el.tagName === "INPUT" && !["checkbox", "radio", "button", "submit"].includes(el.type))
    const role = el.getAttribute("role")
    const name = nameFn(el) || el.getAttribute("placeholder") || ""
    return {
      what: `${el.tagName.toLowerCase()}${role ? `[role=${role}]` : ""} "${name.slice(0, 60)}"`,
      typed,
      name,
      value: "value" in el ? el.value : null,
    }
  })
}

// --- Dialogs ----------------------------------------------------------------

const DIALOG = '[role="dialog"],[role="alertdialog"]'

// Every open dialog, bottom to top, measured: its accessible name, its box, its
// buttons, where its close control and its footer sit, and whether focus is in
// it. "Footer" is the row of the dialog's own action buttons.
async function dialogs(page) {
  return page.evaluate(
    (sel) => {
      const nameFn = window.__checkNameOf
      const box = (el) => {
        const r = el.getBoundingClientRect()
        return { x: r.x, y: r.y, width: r.width, height: r.height, right: r.right, bottom: r.bottom }
      }
      return [...document.querySelectorAll(sel)]
        .filter((d) => !d.hasAttribute("data-closed"))
        .filter((d) => {
          const r = d.getBoundingClientRect()
          return r.width > 1 && r.height > 1
        })
        .map((d) => {
          const buttons = [...d.querySelectorAll("button,a[href]")]
            .filter((b) => b.getBoundingClientRect().width > 0)
            .map((b) => ({
              name: nameFn(b),
              disabled: b.disabled || b.getAttribute("aria-disabled") === "true",
              box: box(b),
            }))
          const footer = d.querySelector('[data-slot="modal-footer"]')
          const header = d.querySelector('[data-slot="modal-header"]')
          return {
            name: nameFn(d),
            box: box(d),
            buttons,
            footer: footer ? box(footer) : null,
            header: header ? box(header) : null,
            focusInside: d.contains(document.activeElement),
            fields: [...d.querySelectorAll("input:not([type=checkbox]):not([type=radio]),textarea")]
              .filter((f) => f.getBoundingClientRect().width > 0)
              .map((f) => ({ name: nameFn(f) || f.getAttribute("placeholder") || "", value: f.value })),
          }
        })
    },
    DIALOG,
  )
}

async function topDialog(page) {
  return (await dialogs(page)).at(-1) || null
}

// Waits for a dialog whose name matches, on top, and returns it measured once it
// has held still for two frames (a card animates in).
async function waitForDialog(dev, name, { timeout = 6000 } = {}) {
  const re = new RegExp(name)
  let last = null
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const top = await topDialog(dev.page)
    if (top && re.test(top.name)) {
      await frames(dev.page)
      const settled = await topDialog(dev.page)
      if (settled && Math.abs(settled.box.y - top.box.y) < 0.5 && Math.abs(settled.box.height - top.box.height) < 0.5) {
        return settled
      }
    }
    last = top
    await sleep(80)
  }
  fail(`no dialog named /${name}/ opened; ${last ? `the dialog on top is "${last.name}"` : "no dialog is open"}`)
}

// Gone from the page, not merely marked closed: a dialog keeps focus through its
// exit animation and hands it back only once it is removed.
async function waitForNoDialog(dev, what, { timeout = 5000 } = {}) {
  const gone = async () =>
    (await dialogs(dev.page)).length === 0 &&
    (await dev.page.evaluate((sel) => document.querySelectorAll(sel).length === 0, DIALOG))
  await until(gone, `${what} to close`, { timeout }).catch(
    async () => {
      const open = (await dialogs(dev.page)).map((d) => `"${d.name}"`).join(", ")
      fail(`${what} did not close; still open: ${open || "a dialog that never finished leaving"}`)
    },
  )
  await frames(dev.page)
}

// One dialog gone, by name, where another one stays (the editor is a dialog of
// its own, and a dialog opened in it closes back onto it).
async function waitForDialogGone(dev, name, what, { timeout = 5000 } = {}) {
  const re = new RegExp(name)
  await until(
    () =>
      dev.page.evaluate(
        (sel, source) => ![...document.querySelectorAll(sel)].some((d) => new RegExp(source).test(window.__checkNameOf(d))),
        DIALOG,
        re.source,
      ),
    `${what} to close`,
    { timeout },
  ).catch(() => fail(`${what} did not close`))
  await frames(dev.page)
}

// --- Menus ------------------------------------------------------------------

// The pane's own ⋯ (the header's on a computer, the flap's on a phone) is the
// last "Session actions" on screen; the sidebar rows' come first.
async function paneMenu(dev, ...items) {
  await press(dev, { name: "^Session actions$", selector: "button", last: true })
  for (const item of items) await press(dev, { name: item, selector: "[role=menuitem]" })
}

// Waits until this device drives the pane on screen, pressing Take over when
// the card is up. A device arriving right after another one closed can still
// meet that one's card, since the server hears about a closed page a moment
// later, so every check that types into a pane or changes its agent asks here.
async function drive(dev, { timeout = 15000 } = {}) {
  const deadline = Date.now() + timeout
  let calm = 0
  let seen = ""
  while (Date.now() < deadline) {
    const state = await dev.page.evaluate(() => {
      const shown = (el) => el.getBoundingClientRect().width > 0
      const card = [...document.querySelectorAll("button")].some((b) => /^Take over$/.test((b.textContent || "").trim()) && shown(b))
      const pane = document.querySelector('[data-testid="terminal-pane"]')
      const text = (pane ? pane.textContent : "").replace(/\s+/g, " ")
      const covered = /Attaching…|Reconnecting…|Launching terminal…|Connection lost\./.test(text)
      const terminal = document.querySelector('[data-testid="terminal-container"]')
      return { card, covered, terminal: Boolean(terminal && shown(terminal)) }
    })
    seen = JSON.stringify(state)
    if (state.card) {
      calm = 0
      await press(dev, { name: "^Take over$", selector: "button" })
      await sleep(300)
      continue
    }
    calm = !state.covered && state.terminal ? calm + 1 : 0
    if (calm >= 3) return
    await sleep(150)
  }
  fail(`this ${dev.kind} never came to drive the pane on screen within ${timeout / 1000}s (${seen})`)
}

// --- The workspace ----------------------------------------------------------

async function agentId(title) {
  const found = (await lib.agents())[title]
  if (!found) fail(`the seeded workspace has no agent named ${title}; rerun with a fresh seed`)
  return found.id
}

const agentHash = (id) => `#/agent/${id}`

// --- Geometry ---------------------------------------------------------------

const near = (a, b, slack = 1) => Math.abs(a - b) <= slack
const rect = (b) => `${Math.round(b.x)},${Math.round(b.y)} ${Math.round(b.width)}x${Math.round(b.height)}`

module.exports = {
  BASE,
  CheckFailure,
  DIALOG,
  OTHER_DEVICE_UA,
  agentHash,
  agentId,
  back,
  closeBrowsers,
  device,
  dialogs,
  expect,
  fail,
  focused,
  frames,
  serverStatusRaised,
  goto,
  hover,
  lib,
  locate,
  near,
  paneMenu,
  press,
  rect,
  sleep,
  dismissToasts,
  drive,
  EDITOR_READY,
  openEditor,
  topDialog,
  until,
  waitFor,
  waitForDialog,
  waitForDialogGone,
  waitForNoDialog,
}
