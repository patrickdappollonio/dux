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

// Set when the run is interrupted, so a check still under way stops at its next
// wait or press instead of changing anything after its clean-up has run.
let halted = false

function halt(on = true) {
  halted = on
}

function stopIfHalted() {
  if (halted) fail("the run was interrupted")
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
      ownSignals: true,
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
  await opened.page.evaluateOnNewDocument(installEventsWatch)
  const dev = { kind, phone, context, log: ctx.log, ...opened }
  opened.page.on("pageerror", (error) => ctx.log(`page error on the ${kind}: ${error.message}`))
  ctx.devices.push(dev)
  return dev
}

// --- Waiting ---------------------------------------------------------------

// What a probe answers while its condition does not hold yet: nothing, false, or
// `{ ok: false, seen }`. Anything else is the answer, 0 and "" included, so a
// toast at the very top of the screen is a position rather than a "not yet".
const notYet = (value) =>
  value === null || value === undefined || value === false || (typeof value === "object" && value.ok === false)

// Polls `probe`, run in the page, until it answers, and fails with `what` and
// the last value seen.
async function waitFor(page, probe, what, { timeout = 8000, gap = 100, arg } = {}) {
  const deadline = Date.now() + timeout
  let last
  for (;;) {
    stopIfHalted()
    last = await page.evaluate(probe, arg)
    if (!notYet(last)) return last
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
    stopIfHalted()
    last = await read()
    if (!notYet(last)) return last
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
// no-password banner is dismissed the way the scenes dismiss it, and the
// statuses the server replays to every page that opens are let in and closed
// (see `settleArrival`).
async function goto(dev, hash = "", { root = "/", ready = WORKSPACE_READY, what = "the workspace to load" } = {}) {
  // Through a blank page, so every goto is a fresh load: an address differing
  // from the current one only after "#" is otherwise a same-document jump that
  // leaves the last step's menus, history entries and focus where they were.
  stopIfHalted()
  await dev.page.goto("about:blank")
  await dev.page.goto(BASE + root + (hash || ""), { waitUntil: "domcontentloaded", timeout: 30000 })
  await waitFor(dev.page, ready, what, { timeout: 20000 })
  // The preview serves with no password, so the banner comes; it is waited for
  // briefly rather than assumed, since it renders once the config has loaded.
  await waitFor(dev.page, () => document.querySelector('[data-testid="no-auth-banner"]') !== null, "the banner", {
    timeout: 3000,
  }).catch(() => {})
  await lib.dismissNoPasswordBanner(dev.page)
  await settleArrival(dev)
  await frames(dev.page)
}

// Installed into every page before the app runs: watches the events socket for
// the statuses the server replays as it opens. They come between the socket's
// `connected` frame and the first frame of anything else, so the first other
// frame says the replay is over. A busy status in it is followed until its final
// arrives, since that final would raise a toast later.
function installEventsWatch() {
  const Native = window.WebSocket
  window.WebSocket = class extends Native {
    constructor(url, protocols) {
      super(url, protocols)
      if (!/\/ws\/events(\?|$)/.test(new URL(String(url), location.href).pathname + "?")) return
      const watch = { open: false, done: false, replayed: [], finals: [] }
      window.__checkEvents = watch
      this.addEventListener("open", () => {
        watch.open = true
      })
      this.addEventListener("message", (event) => {
        if (typeof event.data !== "string") return
        let frame
        try {
          frame = JSON.parse(event.data)
        } catch {
          return
        }
        if (frame.event === "status") {
          if (!watch.done) watch.replayed.push({ key: frame.key, tone: frame.tone, message: frame.message })
          else if (frame.tone !== "busy") watch.finals.push(frame.key)
        } else if (frame.event !== "connected") {
          watch.done = true
        }
      })
    }
  }
}

const ARRIVAL_TIMEOUT_MS = 10000
// How long the toasts on screen must stay the same before the replay's toasts
// count as all drawn: a wait for nothing new to arrive, paid only when the
// server did replay something.
const TOASTS_QUIET_MS = 300

// The server replays every status it finished in the last 30 seconds to each
// page that opens (and, to the first page after a quiet spell, the warnings
// nobody saw). That is the product working, but those toasts are about earlier
// checks, so they are let in and closed as part of arriving. Anything raised
// after this is the check's own business: a toast over a control a check then
// presses fails that check.
async function settleArrival(dev) {
  const { page } = dev
  const replay = await waitFor(
    page,
    () => {
      const w = window.__checkEvents
      if (!w) return { ok: false, seen: "no events socket opened" }
      if (!w.done) return { ok: false, seen: `the events socket ${w.open ? "open" : "not open yet"}, ${w.replayed.length} replayed status(es) so far` }
      return w.replayed
    },
    "the events socket to finish replaying recent statuses",
    { timeout: ARRIVAL_TIMEOUT_MS },
  )
  if (replay.length === 0) return
  const busy = replay.filter((s) => s.tone === "busy" && s.key)
  if (busy.length) {
    await waitFor(
      page,
      (keys) => keys.every((k) => window.__checkEvents.finals.includes(k)) || { ok: false, seen: JSON.stringify(window.__checkEvents.finals) },
      `the server's replayed busy status(es) to finish (${busy.map((s) => `"${s.message}"`).join(", ")}: something started before this check is still running)`,
      { timeout: ARRIVAL_TIMEOUT_MS, arg: busy.map((s) => s.key) },
    )
  }
  const deadline = Date.now() + ARRIVAL_TIMEOUT_MS
  let seen = null
  let since = Date.now()
  while (Date.now() < deadline) {
    const now = JSON.stringify(await liveToasts(page))
    if (now !== seen) {
      seen = now
      since = Date.now()
    } else if (Date.now() - since >= TOASTS_QUIET_MS) {
      if (now === "[]") return
      await closeToasts(dev)
      seen = null
    }
    await sleep(50)
  }
  fail(
    `toasts kept arriving for ${ARRIVAL_TIMEOUT_MS / 1000}s after the page opened (the server replayed ${replay
      .map((s) => `"${s.message}"`)
      .join(", ")}; on screen: ${seen})`,
  )
}

// The text of every toast on screen that is not on its way out.
async function liveToasts(page) {
  return page.evaluate(() =>
    [...document.querySelectorAll("[data-sonner-toast]")]
      .filter((t) => t.getAttribute("data-removed") !== "true" && t.getBoundingClientRect().height > 0)
      .map((t) => t.textContent.replace(/\s+/g, " ").trim()),
  )
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

// The browser's Back, waited out by the history move it makes. What the page
// then shows is the caller's to wait for.
async function back(dev, { timeout = 3000 } = {}) {
  const moved = await dev.page
    .evaluate(
      (ms) =>
        new Promise((done) => {
          const timer = setTimeout(() => done(false), ms)
          addEventListener("popstate", () => {
            clearTimeout(timer)
            done(true)
          }, { once: true })
          history.back()
        }),
      timeout,
    )
    // Back that leaves the document destroys the page this ran in; where it
    // went is for the caller to read.
    .catch(() => true)
  expect(moved, `the browser's Back moved nothing within ${timeout / 1000}s`)
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
  window.__checkMarks = {}
}

// The centre of the first visible element matching `selector` whose name
// matches `name` (a regex source), inside `within` when given. With `mark`, the
// element is remembered under that name, so a check can later ask whether focus
// is on that very element (`focusIsMarked`) rather than on one named the same.
async function locate(page, { name = null, selector = "button,[role=menuitem]", within = null, nth = 0, last = false, mark = null }) {
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
      if (spec.mark) window.__checkMarks[spec.mark] = el
      el.scrollIntoView({ block: "nearest" })
      const r = el.getBoundingClientRect()
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
    },
    { name, selector, within, nth, last, mark },
  )
}

const specOf = (spec) => (typeof spec === "string" ? { name: spec } : spec)
const described = (s) => `/${s.name}/ (${s.selector || "button,[role=menuitem]"})`

// Where a press or a hover on the element a spec names would land, once that
// element has held still and nothing lies over it. A sheet or a menu slides in,
// and a screen still loading shifts its rows, so the point is read again until
// it has held still for three readings in a row: a tap on where an item was a
// moment ago lands on something else (measured: the phone editor's tree moved a
// file's row 16px after it was first drawn, and the tap's click went to the
// row's container). A control that never holds still, or that something lies
// over, fails the check: a toast over it is exactly the kind of bug a check is
// for, so it is not closed here.
async function aim(dev, spec, verb, { timeout = 5000 } = {}) {
  stopIfHalted()
  const s = specOf(spec)
  let at = null
  const deadline = Date.now() + timeout
  while (!at && Date.now() < deadline) {
    at = await locate(dev.page, s)
    if (!at) await sleep(100)
  }
  if (!at) fail(`there is nothing to ${verb} named ${described(s)}`)
  const first = at
  let stillFor = 0
  for (let i = 0; i < 40 && stillFor < 3; i++) {
    await frames(dev.page)
    await sleep(40)
    const now = await locate(dev.page, s)
    if (!now) fail(`the control named ${described(s)} went away while waiting for it to hold still before a ${verb}`)
    stillFor = Math.abs(now.x - at.x) < 0.5 && Math.abs(now.y - at.y) < 0.5 ? stillFor + 1 : 0
    at = now
  }
  if (stillFor < 3) {
    fail(
      `the control named ${described(s)} never held still before a ${verb}: it moved from ${Math.round(first.x)},${Math.round(first.y)} and was still moving at ${Math.round(at.x)},${Math.round(at.y)}`,
    )
  }
  const cover = await coveredBy(dev.page, at, s)
  if (cover && cover.toast) {
    fail(`a toast reading "${cover.what}" lies over the control named /${s.name}/, so a ${verb} would land on the toast`)
  }
  if (cover) fail(`the control named /${s.name}/ is covered by ${cover.what}, so a ${verb} would land there instead`)
  return at
}

// A real press, by mouse on a computer and by a tap on a phone, on the element
// a spec names. A press is what a person does; element.click() skips the
// pointer events base-ui menus act on.
async function press(dev, spec, options) {
  const at = await aim(dev, spec, "press", options)
  if (dev.phone) await dev.page.touchscreen.tap(at.x, at.y)
  else await dev.page.mouse.click(at.x, at.y)
  await frames(dev.page)
}

async function hover(dev, spec, options) {
  const at = await aim(dev, spec, "hover", options)
  await dev.page.mouse.move(at.x, at.y)
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
      // A disabled control's explanation wrapper takes the pointer for it (it is
      // what raises the reason), so a wrapper holding the named control and no
      // other is the control.
      const controls = [...hit.querySelectorAll("button,a[href],input,textarea,select,[role=menuitem]")]
      if (controls.length === 1 && controls[0].matches(sel) && (!re || re.test(window.__checkNameOf(controls[0])))) return null
      return { toast: false, what: `${hit.tagName.toLowerCase()} "${(window.__checkNameOf(hit) || "").slice(0, 60)}"` }
    },
    at,
    { name: spec.name === undefined ? null : spec.name, selector: spec.selector },
  )
}

// Closes every toast on screen with its own Close button, frontmost first, each
// waited out until it has gone. Housekeeping rather than something under test,
// so the button is clicked where it is: a stack of toasts still settling can
// leave a Close under its neighbour for a moment.
async function closeToasts(dev, { timeout = 6000 } = {}) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    const closing = await dev.page.evaluate(() => {
      const live = [...document.querySelectorAll("[data-sonner-toast]")].filter((t) => t.getAttribute("data-removed") !== "true")
      const toast = live.at(-1)
      if (!toast) return null
      const text = toast.textContent.replace(/\s+/g, " ").trim().slice(0, 120)
      const b = [...toast.querySelectorAll("button")].find((x) => window.__checkNameOf(x) === "Close toast")
      if (!b) return { text, missing: true }
      window.__checkClosing = toast
      b.click()
      return { text }
    })
    if (!closing) break
    if (closing.missing) fail(`the toast reading "${closing.text}" has no Close button`)
    await waitFor(
      dev.page,
      () => !window.__checkClosing.isConnected || window.__checkClosing.getAttribute("data-removed") === "true",
      `the toast reading "${closing.text}" to close`,
      { timeout: 2000 },
    )
  }
  await waitFor(dev.page, () => document.querySelector("[data-sonner-toast]") === null, "the closed toasts to leave the page", {
    timeout: Math.max(1000, deadline - Date.now()),
  })
  await frames(dev.page)
}

// For a check that closes toasts it raised itself before pressing on: `what`
// names them, and is what a reader of the check sees it is allowed to clear.
async function dismissOwnToasts(dev, what) {
  if (typeof what !== "string" || !what) throw new Error("dismissOwnToasts needs to be told which of the check's own toasts it closes")
  await closeToasts(dev)
}

// Whether focus is on the very element remembered under `mark` (see `locate`).
async function focusIsMarked(page, mark) {
  return page.evaluate((m) => {
    const el = window.__checkMarks[m]
    return {
      same: Boolean(el) && document.activeElement === el,
      marked: el ? `${el.tagName.toLowerCase()} "${window.__checkNameOf(el).slice(0, 60)}"${el.isConnected ? "" : " (no longer on the page)"}` : "nothing",
    }
  }, mark)
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

// The reason a disabled control in the top dialog gives when pointed at: hovers
// it and reads the tooltip whose trigger is the control's own wrapper (a trigger
// carries `data-popup-open` while its popup is open). The tooltip names no
// trigger of its own, so it counts only while it is the one tooltip on screen.
async function reasonFor(dev, name) {
  const read = () =>
    dev.page.evaluate((wanted) => {
      const dialog = [...document.querySelectorAll('[role="dialog"],[role="alertdialog"]')].at(-1)
      const control = dialog && [...dialog.querySelectorAll("button")].find((b) => window.__checkNameOf(b) === wanted)
      if (!control) return { ok: false, seen: `the top dialog has no ${wanted} button` }
      let trigger = null
      for (let el = control; el && el !== dialog; el = el.parentElement) {
        if (el.hasAttribute("data-popup-open")) trigger = el
      }
      const tips = [...document.querySelectorAll('[data-slot="tooltip-content"]')]
        .filter((el) => el.getBoundingClientRect().width > 0)
        .map((el) => el.textContent.replace(/\s+/g, " ").trim())
      if (!trigger) return { ok: false, seen: `${wanted} opened no tooltip (tooltips on screen: ${JSON.stringify(tips)})` }
      if (tips.length !== 1 || !tips[0]) return { ok: false, seen: `${wanted}'s tooltip is open, and the tooltips on screen read ${JSON.stringify(tips)}` }
      return tips[0]
    }, name)
  await waitFor(dev.page, () => !document.querySelector('[data-slot="tooltip-content"]'), `no tooltip before pointing at ${name}`, {
    timeout: 2000,
  })
  await hover(dev, { name: `^${name}$`, selector: "button", within: DIALOG })
  return until(read, `a reason for the disabled ${name}`, { timeout: 3000 }).catch(async () =>
    fail(`the disabled ${name} should give a reason when pointed at; ${(await read()).seen}`),
  )
}

// --- Menus ------------------------------------------------------------------

// The pane's own ⋯ (the header's on a computer, the flap's on a phone) is the
// last "Session actions" on screen; the sidebar rows' come first. It is
// remembered as the "opener", the control a dialog opened from its menu hands
// focus back to.
async function paneMenu(dev, ...items) {
  await press(dev, { name: "^Session actions$", selector: "button", last: true, mark: "opener" })
  for (const item of items) await press(dev, { name: item, selector: "[role=menuitem]" })
}

// What the pane on screen shows a device: the take-over card (and its title), a
// cover while it attaches or reconnects, or the terminal itself.
async function paneState(page) {
  return page.evaluate(() => {
    const shown = (el) => el.getBoundingClientRect().width > 0
    const button = [...document.querySelectorAll("button")].find((b) => /^Take over$/.test((b.textContent || "").trim()) && shown(b))
    // The card's button sits in its content, under the card with its title.
    const card = button && button.parentElement ? button.parentElement.parentElement : null
    const pane = document.querySelector('[data-testid="terminal-pane"]')
    const text = (pane ? pane.textContent : "").replace(/\s+/g, " ")
    const covered = /Attaching…|Reconnecting…|Launching terminal…|Connection lost\./.test(text)
    const terminal = document.querySelector('[data-testid="terminal-container"]')
    return {
      card: button ? (card ? card.textContent.replace(/\s+/g, " ").trim().slice(0, 160) : "Take over") : null,
      covered,
      terminal: Boolean(terminal && shown(terminal)),
    }
  })
}

// Waits until this device drives the pane on screen: the terminal is shown with
// no cover over it. A device arriving at a pane nobody drives claims it, and the
// runner waits after every check until no device drives anything, so the
// take-over card here means another device is driving where none should be,
// and fails the check. A check that means to take a pane over from another of
// its devices says so with `takeOver`.
async function drive(dev, { timeout = 15000 } = {}) {
  const deadline = Date.now() + timeout
  let calm = 0
  let seen = ""
  while (Date.now() < deadline) {
    const state = await paneState(dev.page)
    seen = JSON.stringify(state)
    if (state.card) {
      fail(`this ${dev.kind} should come to drive the pane on screen, with no other device driving it; the take-over card is up instead ("${state.card}")`)
    }
    calm = !state.covered && state.terminal ? calm + 1 : 0
    if (calm >= 3) return
    await sleep(150)
  }
  fail(`this ${dev.kind} never came to drive the pane on screen within ${timeout / 1000}s (${seen})`)
}

// Takes the pane on screen over from another device: waits for the take-over
// card, presses its button, and waits until this device drives the pane.
async function takeOver(dev, { timeout = 10000 } = {}) {
  await until(async () => (await paneState(dev.page)).card, `the take-over card on this ${dev.kind}`, { timeout }).catch(async () =>
    fail(`this ${dev.kind} should meet the take-over card, another device driving the pane; it shows ${JSON.stringify(await paneState(dev.page))}`),
  )
  await press(dev, { name: "^Take over$", selector: "button" })
  const deadline = Date.now() + timeout
  while ((await paneState(dev.page)).card && Date.now() < deadline) await sleep(100)
  await drive(dev, { timeout })
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
  halt,
  frames,
  goto,
  hover,
  lib,
  locate,
  near,
  paneMenu,
  press,
  reasonFor,
  rect,
  sleep,
  dismissOwnToasts,
  drive,
  focusIsMarked,
  installEventsWatch,
  liveToasts,
  takeOver,
  EDITOR_READY,
  openEditor,
  topDialog,
  until,
  waitFor,
  waitForDialog,
  waitForDialogGone,
  waitForNoDialog,
}
