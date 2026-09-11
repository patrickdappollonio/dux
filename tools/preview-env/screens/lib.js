// Shared plumbing for the committed screenshot scenes: one browser contract and
// one REST client, so a scene file holds only what makes its own picture.
//
// The launch contract is shot.js's, for the same measured reasons: the scale is
// a browser flag, the CDP metrics override is sent with deviceScaleFactor 0, and
// SwiftShader is asked for by name so the terminal's webgl canvas is not
// captured black.
const { spawnSync } = require("child_process")
const path = require("path")
const { captureProblem, decodePng, inkRatio, measureInk } = require("./ink.js")

// Required inside `open` rather than here: loading a scene must cost nothing but
// node, so the website suite's loop test can read every scene without this
// directory's dependencies being installed.
const puppeteer = () => require("puppeteer-core")

const PORT = process.env.DUX_PORT || "8790"
const BASE = `http://127.0.0.1:${PORT}`

const DESKTOP = { width: 1440, height: 900 }
const PHONE = { width: 390, height: 844 }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// --- REST ------------------------------------------------------------------

async function api(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: body ? { "content-type": "application/json" } : {},
    body: body ? JSON.stringify(body) : undefined,
  })
  const text = await res.text()
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${text.slice(0, 400)}`)
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

const get = (path) => api("GET", path)

// The fake provider reads its fixture out of the global environment, so the
// scene picked for an agent is whatever was set when its process was spawned.
const setFixture = (name) => api("PUT", "/api/v1/global-env", { env: { DUX_FAKE_FIXTURE: name } })

// Every agent by its display title, which is the name the seed gave it.
async function agents() {
  const list = await get("/api/v1/sessions")
  return Object.fromEntries(list.map((s) => [s.title, s]))
}

async function project(path) {
  const list = await get("/api/v1/projects")
  const found = list.find((p) => p.path === path)
  if (!found) throw new Error(`no project seeded at ${path}`)
  return found
}

// Restart one agent on a chosen fixture and give it a fixed number of seconds to
// stream. A pane that has been streaming for minutes is a wall of test-batch
// lines; the committed shots show a session that just came up.
async function freshen(id, seconds, fixture = "working") {
  await setFixture(fixture)
  await api("POST", `/api/v1/sessions/${id}/kill`)
  await sleep(1500)
  await api("POST", `/api/v1/sessions/${id}/reconnect`)
  await sleep(seconds * 1000)
}

// --- Inside the container ---------------------------------------------------

// Git remotes and worktree files have no REST route, so the few pieces of scene
// that are filesystem facts are seeded through the container's own shell.
// Docker access is resolved the way up.sh resolves it: directly when this shell
// has it, through `sg docker` when the group was added without a re-login.
const composeDir = path.resolve(__dirname, "..")
let directDocker = null

function containerSh(script) {
  if (directDocker === null) {
    directDocker = spawnSync("docker", ["info"], { stdio: "ignore" }).status === 0
  }
  const args = ["compose", "exec", "-T", "dux", "sh", "-euc", script]
  const result = directDocker
    ? spawnSync("docker", args, { cwd: composeDir, encoding: "utf8" })
    : spawnSync(
        "sg",
        [
          "docker",
          "-c",
          `cd ${JSON.stringify(composeDir)} && docker compose exec -T dux sh -euc ${JSON.stringify(script)}`,
        ],
        { encoding: "utf8" },
      )
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || "").trim()
    throw new Error(`container command failed: ${detail || result.status}`)
  }
  return result.stdout
}

// --- Browser ---------------------------------------------------------------

function launchArgs(width, height) {
  return [
    "--no-sandbox",
    "--disable-dev-shm-usage",
    "--force-color-profile=srgb",
    "--hide-scrollbars",
    "--force-device-scale-factor=2",
    `--window-size=${width},${height}`,
    "--use-gl=angle",
    "--use-angle=swiftshader",
    "--enable-unsafe-swiftshader",
  ]
}

async function open({ mobile = false, width, height } = {}) {
  const preset = mobile ? PHONE : DESKTOP
  const w = width || preset.width
  const h = height || preset.height
  const browser = await puppeteer().launch({
    executablePath: process.env.CHROME,
    headless: "new",
    defaultViewport: null,
    args: launchArgs(w, h),
  })
  const page = await browser.newPage()
  const cdp = await page.createCDPSession()
  const metrics = { width: w, height: h, deviceScaleFactor: 0, mobile }
  await cdp.send("Emulation.setDeviceMetricsOverride", metrics)
  if (mobile) {
    await cdp.send("Emulation.setTouchEmulationEnabled", { enabled: true, maxTouchPoints: 5 })
    await cdp.send("Emulation.setEmitTouchEventsForMouse", {
      enabled: true,
      configuration: "mobile",
    })
  }
  // A window opened at a size the platform will not give can write the window's
  // pixels rather than the emulated viewport's, so the viewport is checked
  // immediately before every capture and the override re-sent when it has
  // drifted. Only when: re-sending it is a viewport change as far as the page is
  // concerned, and a base-ui menu closes on one, which silently turned a picture
  // of an open menu into a picture of the strip it hangs off.
  const reassert = async () => {
    const now = await page.evaluate(() => ({
      width: window.innerWidth,
      height: window.innerHeight,
    }))
    if (now.width === w && now.height === h) return
    await cdp.send("Emulation.setDeviceMetricsOverride", metrics)
  }
  return { browser, page, cdp, reassert, width: w, height: h }
}

async function goto(page, hash) {
  await page.goto(BASE + "/" + (hash || ""), { waitUntil: "networkidle0", timeout: 45000 })
  await sleep(1500)
}

// A capture opens a real connection to the PTY, so it arrives as a watcher and
// the full-pane card covers the terminal until this presses its button.
async function takeOver(page) {
  for (let i = 0; i < 20; i++) {
    const pressed = await page.evaluate(() => {
      const btn = [...document.querySelectorAll("button")].find((b) =>
        /take over/i.test(b.textContent || ""),
      )
      if (!btn) return false
      btn.click()
      return true
    })
    if (pressed) {
      await sleep(3000)
      return true
    }
    await sleep(500)
  }
  return false
}

// Toasts stack over the bottom of the page and are never part of a scene.
async function clearToasts(page) {
  await page.evaluate(() => {
    document.querySelectorAll("[data-sonner-toaster]").forEach((n) => n.remove())
  })
  await sleep(200)
}

// --- Geometry --------------------------------------------------------------

// Bounding box of every element matching any selector, unioned, in CSS pixels
// and clamped to the viewport.
async function boxOf(page, selectors, pad = 0, bounds = DESKTOP) {
  const r = await page.evaluate((sels) => {
    let x0 = Infinity
    let y0 = Infinity
    let x1 = -Infinity
    let y1 = -Infinity
    for (const s of sels) {
      for (const el of document.querySelectorAll(s)) {
        const b = el.getBoundingClientRect()
        if (b.width === 0 && b.height === 0) continue
        x0 = Math.min(x0, b.x)
        y0 = Math.min(y0, b.y)
        x1 = Math.max(x1, b.right)
        y1 = Math.max(y1, b.bottom)
      }
    }
    return Number.isFinite(x0) ? { x0, y0, x1, y1 } : null
  }, selectors)
  if (!r) throw new Error(`no elements for ${selectors.join(", ")}`)
  const x = Math.max(0, Math.round(r.x0 - pad))
  const y = Math.max(0, Math.round(r.y0 - pad))
  return {
    x,
    y,
    width: Math.min(bounds.width - x, Math.round(r.x1 + pad) - x),
    height: Math.min(bounds.height - y, Math.round(r.y1 + pad) - y),
  }
}

// --- Pointer helpers -------------------------------------------------------

// Click by aria-label; nth picks among equal labels.
async function clickLabel(page, label, nth = 0) {
  const ok = await page.evaluate(
    (l, n) => {
      const els = [...document.querySelectorAll(`[aria-label="${l}"]`)]
      if (!els[n]) return false
      els[n].click()
      return true
    },
    label,
    nth,
  )
  if (!ok) throw new Error(`no element with aria-label ${label} #${nth}`)
  await sleep(700)
}

// A real pointer click: base-ui menu items act on pointer events, not on the
// synthetic click an element.click() dispatches.
async function clickText(page, source, sel = "button,[role=menuitem]") {
  const box = await page.evaluate(
    (s, r) => {
      const rx = new RegExp(r, "i")
      const el = [...document.querySelectorAll(s)].find((b) => rx.test(b.textContent || ""))
      if (!el) return null
      const b = el.getBoundingClientRect()
      return { x: b.x + b.width / 2, y: b.y + b.height / 2 }
    },
    sel,
    source,
  )
  if (!box) throw new Error(`no ${sel} matching ${source}`)
  await page.mouse.click(box.x, box.y)
  await sleep(900)
}

// --- Staging ---------------------------------------------------------------

// The sidebar order every scene is shot against. The active sort floats the
// working and needs-you agents together in reverse creation order, which puts
// the agent that needs you last; the screenshots pin an order instead.
const SIDEBAR_ORDER = [
  "design-notes",
  "review-billing",
  "polish-onboarding",
  "refactor-cache",
  "add-rate-limits",
  "fix-login-redirect",
]

// Three stagings, because three groups of screenshots are about different
// things. "all-working" is the busy workspace: every agent works and one needs
// you. "twin" is the workspace both front ends are shown side by side in, where
// half the agents are idle. "standalone" is "all-working" with the folder agent
// running the transcript provider instead of the fake one, which is the only
// scene that wants a scripted session rather than a streaming fixture.
//
// A staging names a fixture per agent, and optionally the provider its first tab
// runs; the provider defaults to `fake`, whose fixtures are what make working
// and idle states shootable at all.
const BUSY = {
  // The folder agent is seeded on the transcript provider for its own
  // screenshot, where a ticking counter always reads Working. Staged among the
  // others it runs the fake provider, so its state follows the fixture like
  // every other agent's; every other agent keeps the providers the seed gave it.
  "design-notes": { fixture: "working", provider: "fake" },
  "review-billing": { fixture: "attention" },
  "polish-onboarding": { fixture: "working" },
  "refactor-cache": { fixture: "working" },
  "add-rate-limits": { fixture: "working" },
  "fix-login-redirect": { fixture: "working" },
}

const STAGINGS = {
  "all-working": BUSY,
  twin: {
    ...BUSY,
    "design-notes": { fixture: "steady", provider: "fake" },
    "review-billing": { fixture: "steady" },
    "polish-onboarding": { fixture: "steady" },
  },
  standalone: { ...BUSY, "design-notes": { fixture: "working", provider: "claude" } },
}

// The agent the busy stagings leave waiting on you, and the fixture that rings
// the bell it waits with.
const ATTENTION_AGENT = "review-billing"

// Re-arm that agent's needs-you flag, on the same path `stage` relights an agent
// on: the flag clears the moment someone looks at the pane, so any scene that
// opens this agent leaves the workspace without one. A scene that is ABOUT the
// flag calls this before it shoots rather than trusting whatever ran before it,
// and a scene that clears it puts it back afterwards.
async function armAttention(title = ATTENTION_AGENT, timeoutMs = 40000) {
  const by = await agents()
  const session = by[title]
  if (!session) throw new Error(`the seed has not created ${title}`)
  await setFixture("attention")
  await api("POST", `/api/v1/sessions/${session.id}/kill`)
  await sleep(1200)
  await api("POST", `/api/v1/sessions/${session.id}/reconnect`)

  // Wait for the flag itself, not for the restart: the fixture rings the bell
  // once, shortly after it comes up, and resetting the global environment before
  // the provider has been spawned would hand the next spawn the wrong fixture.
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const now = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
    if (now && now.needs_attention) {
      await setFixture("working")
      return
    }
    await sleep(500)
  }
  await setFixture("working")
  throw new Error(`${title} never raised its needs-you flag`)
}

// The word that agent's row wears once the flag is up. A scene about the flag
// asks for it through `expectStateWord`, which is where every guard lives now.
const ATTENTION_WORD = "Needs you"

// Relight every agent on the fixture its staging asks for. The fixture is read
// at spawn, so each agent is stopped and started again with the global
// environment holding the value meant for it.
async function stage(name) {
  const want = STAGINGS[name]
  if (!want) throw new Error(`unknown staging ${name}; have ${Object.keys(STAGINGS).join(", ")}`)
  const by = await agents()

  for (const title of SIDEBAR_ORDER) {
    const session = by[title]
    if (!session) throw new Error(`the seed has not created ${title}`)
    // Only an agent whose staging names a provider has its first tab
    // retargeted. Everything else keeps the providers the seed gave it, which is
    // what leaves refactor-cache carrying one tab per provider for the tab-strip
    // shots.
    const provider = want[title].provider
    if (provider && session.tabs[0].provider !== provider) {
      await api("PATCH", `/api/v1/sessions/${session.id}/tabs/${session.tabs[0].id}`, { provider })
    }
    await setFixture(want[title].fixture)
    await api("POST", `/api/v1/sessions/${session.id}/kill`)
    await sleep(1200)
    await api("POST", `/api/v1/sessions/${session.id}/reconnect`)
    await sleep(2500)
    const now = (await get("/api/v1/sessions")).find((s) => s.id === session.id)
    for (const tab of now.tabs) {
      if (!tab.has_live_process) {
        await api("POST", `/api/v1/sessions/${session.id}/tabs/${tab.id}/start`)
        await sleep(1200)
      }
    }
    console.log(`  ${title.padEnd(20)} -> ${want[title].fixture}`)
  }
  await setFixture("working")

  await api("POST", "/api/v1/sessions/reorder-global", {
    session_ids: SIDEBAR_ORDER.map((t) => by[t].id),
  })
  await sleep(800)
}

// --- The launcher corner ---------------------------------------------------

async function clickBox(page, box, label) {
  if (!box) throw new Error(`no ${label}`)
  await page.mouse.click(box.x, box.y)
  await sleep(1200)
}

// The corner's filled verb, not the Agents-header "+" that shares its label.
async function openNewAgent(page) {
  const box = await page.evaluate(() => {
    const b = [...document.querySelectorAll("button")].find(
      (x) => /^\s*New agent\s*$/.test(x.textContent || "") && x.getBoundingClientRect().width > 40,
    )
    if (!b) return null
    const r = b.getBoundingClientRect()
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
  })
  await clickBox(page, box, "New agent verb")
}

// The launcher corner's grouped ellipsis, which is the constant home of every
// other way to create something.
async function openMoreWays(page) {
  const box = await page.evaluate(() => {
    const b = [...document.querySelectorAll('[aria-label="More ways to create"]')].find(
      (x) => x.getBoundingClientRect().width > 0,
    )
    if (!b) return null
    const r = b.getBoundingClientRect()
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 }
  })
  await clickBox(page, box, "More ways to create trigger")
}

// --- Refusal guards --------------------------------------------------------
//
// A capture that shows the wrong thing is worse than one that never happened:
// the docs carry the picture and nobody looks at it again. Three went out at
// once (a pane stuck on "Attaching…", a blank sidebar, a menu that had closed
// itself under a take-over card) and the tool reported success for all three,
// because nothing it did was about what the picture contains.
//
// So every scene states what its own caption promises, immediately before the
// shutter. A guard reads the live page, returns nothing when the promise holds,
// and throws one sentence naming the scene and what was missing when it does
// not; run.js writes no PNG for a scene whose guard threw.

// Set by run.js before each scene, so a refusal says which picture was refused
// without every call site repeating its own name. It also clears the replay
// below, which belongs to one scene at a time.
let sceneUnderShot = "scene"
const setSceneName = (name) => {
  sceneUnderShot = name
  guardReplay = []
  guardsAsked = 0
  captureClip = null
}

function refuse(what) {
  // Which pass refused matters: a scene whose promise held where it was written
  // and broke by the time the shutter opened is a different bug from one that
  // never got there, and they read identically without this.
  const when = replaying ? " (asked again with the shutter open)" : ""
  throw refusal(`${sceneUnderShot}: ${what}${when}`)
}

// A refusal is a statement about the picture; everything else that can go wrong
// here (no Chromium, a selector that never appeared, a socket that died) is the
// tool failing rather than the picture being wrong, and a person reading the
// table needs to know which they are looking at.
function refusal(sentence) {
  const error = new Error(sentence)
  error.refusal = true
  return error
}

const isRefusal = (error) => Boolean(error && error.refusal)

// A guard is a question asked of a live page, and a page settles: a replay
// arrives, a menu finishes opening, a fixture prints its first line. So a guard
// waits a bounded while for its promise to come true and refuses only when it
// never does, saying what it saw last. Waiting is not weakening: the failures
// this exists for (a pane that never attached, a menu that never opened) are
// still there at the end of the window.
const GUARD_SETTLE_MS = 12000
const GUARD_POLL_MS = 500

async function settle(probe) {
  const deadline = Date.now() + GUARD_SETTLE_MS
  let problem = await probe()
  while (problem && Date.now() < deadline) {
    await sleep(GUARD_POLL_MS)
    problem = await probe()
  }
  if (problem) refuse(problem)
}

// Every guard a scene ran, so run.js can ask the same questions again with the
// shutter open. The scene's own call is not the last word: the capture happens
// after the scene returns, and the steps between (re-asserting the viewport,
// parking the animations) are page events a menu closes on. That cost a picture
// of an open tab menu, which came back as a bare strip and passed.
let guardReplay = []
let guardsAsked = 0
let replaying = false

// Wraps a guard so calling it also records the question. The recording is
// suppressed while replaying, or a replay would grow the list it walks.
//
// `once: true` in a guard's options keeps it out of the replay, for the rare
// question whose answer is SUPPOSED to change before the shutter: a scene that
// reads the terminal and then covers it with an overlay would otherwise be asked
// about the terminal again and shown the overlay's pixels.
function recorded(check) {
  return async (...args) => {
    await check(...args)
    const options = args[args.length - 1]
    const once = Boolean(options && typeof options === "object" && options.once)
    if (replaying) return
    guardsAsked += 1
    if (!once) guardReplay.push(() => check(...args))
  }
}

// How many questions this scene asked, the once-only ones included. Zero is a
// scene with nothing checking what it produces, which run.js refuses: a source
// scan for the word cannot tell a call from a mention of one in a comment.
const recordedGuardCount = () => guardsAsked

async function recheckGuards() {
  replaying = true
  try {
    for (const ask of guardReplay) await ask()
  } finally {
    replaying = false
  }
}

// Whitespace is normalized on both sides: the DOM's own line breaks and the
// spaces the UI puts between a glyph and its word are not differences a caption
// is about.
const flatten = (text) => (text || "").replace(/\s+/g, " ").trim()

// The frame the picture is cut from, set by run.js once the scene has handed
// back its clip and null while the scene is still driving. "On screen" in a
// guard means IN THE PICTURE, and a crop is most of the difference: an element
// sitting happily in the viewport but outside the crop is not in the picture the
// caption is about, and the viewport itself is the frame until the crop is
// known.
let captureClip = null
const setCaptureClip = (clip) => {
  captureClip = clip || null
}

// Every visible element matching a selector (and, when given, carrying a
// phrase), measured and clipped to the frame. One scan behind the three text
// guards, so "visible" means the same thing to all of them.
//
// The rule is a real overlap rather than containment: an element wider than the
// crop (a diff editor the crop cuts a band out of) is in the picture, while one
// scrolled out of the viewport overlaps it by nothing at all.
const MIN_VISIBLE_PX = 4

async function visibleMatches(page, { selector = "*", needle = null, slack = null } = {}) {
  const frame = captureClip
  return page.evaluate(
    (sel, want, extra, box, minimum) => {
      const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
      const phrase = want === null ? null : flat(want)
      const clip = box || { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight }
      const out = []
      for (const el of document.querySelectorAll(sel)) {
        if (phrase !== null) {
          const own = flat(el.textContent)
          if (!own.includes(phrase)) continue
          if (extra !== null && own.length > phrase.length + extra) continue
        }
        const r = el.getBoundingClientRect()
        if (r.width < 1 || r.height < 1) continue
        const style = getComputedStyle(el)
        if (style.visibility === "hidden" || Number(style.opacity) < 0.05) continue
        const overlapWidth = Math.min(r.right, clip.x + clip.width) - Math.max(r.x, clip.x)
        const overlapHeight = Math.min(r.bottom, clip.y + clip.height) - Math.max(r.y, clip.y)
        if (overlapWidth < minimum || overlapHeight < minimum) continue
        out.push({ width: r.width, height: r.height, area: r.width * r.height })
      }
      return out
    },
    selector,
    needle,
    slack,
    frame,
    MIN_VISIBLE_PX,
  )
}

// The smallest visible element carrying a phrase, or null. The smallest one is
// the one that OWNS the text: every ancestor up to <body> contains it too, and
// their widths say nothing.
async function findText(page, text, selector) {
  const matches = await visibleMatches(page, { selector, needle: text })
  return matches.reduce((best, one) => (!best || one.area < best.area ? one : best), null)
}

// A phrase the caption promises, in the picture.
async function expectVisibleText(page, text, { selector = "*", what } = {}) {
  await settle(async () => {
    const found = await findText(page, text, selector)
    return found ? null : `${what || `the text "${flatten(text)}"`} is not in the picture`
  })
}

// A visible element, named by what it is rather than by its selector: a scene
// whose subject is a shape (a flap, a pill, an overlay) has no words of its own
// to look for.
async function expectVisible(page, selector, what) {
  await settle(async () => {
    const matches = await visibleMatches(page, { selector })
    return matches.length
      ? null
      : `${what} is not in the picture (nothing visible matches ${selector} inside the frame)`
  })
}

// A wide strip of chrome rather than an incidental mention of the same words:
// the sidebar row's pull-request chip carries the banner's own text at chip
// width, and a picture of the chip is not a picture of the banner.
//
// The WIDEST element that is about this text, which is the strip itself rather
// than the span inside it holding the title. "About" is what bounds the search:
// every ancestor up to <body> contains the words too, so an element saying much
// more than the phrase is a container rather than the banner.
const BANNER_SLACK = 80

async function expectBanner(page, text, { minWidth = 300, selector = "*", what } = {}) {
  await settle(() => bannerProblem(page, text, minWidth, selector, what))
}

async function bannerProblem(page, text, minWidth, selector, what) {
  const matches = await visibleMatches(page, { selector, needle: text, slack: BANNER_SLACK })
  const found = matches.reduce((best, one) => (!best || one.width > best.width ? one : best), null)
  if (!found) return `${what || `the banner reading "${flatten(text)}"`} is not in the picture`
  if (found.width < minWidth) {
    return `${what || `"${flatten(text)}"`} is only ${Math.round(found.width)}px wide, which is a chip rather than a banner`
  }
  return null
}

// What a field holds, which is its value rather than anything in the DOM.
async function expectFieldValue(page, selector, text) {
  await settle(async () => {
    const values = await page.evaluate(
      (sel) => [...document.querySelectorAll(sel)].map((el) => el.value || ""),
      selector,
    )
    if (values.some((value) => flatten(value).includes(flatten(text)))) return null
    return `no ${selector} holds "${flatten(text)}"; they hold ${JSON.stringify(values.map(flatten))}`
  })
}

// --- The pane --------------------------------------------------------------

// Every wording the pane's cover can carry. The card is matched on its button
// rather than on its words, because an agent row's menu carries a sentence
// about taking over as ordinary prose.
const COVER_WORDINGS = [
  ["Attaching…", "the pane is still attaching"],
  ["Reconnecting…", "the pane is still reconnecting"],
  ["Launching terminal…", "the terminal has not come up"],
  ["Connection lost.", "the pane is showing the reconnect box"],
  ["Still waiting for the terminal's screen.", "the pane is showing the reconnect box"],
]

// Nothing is covering the terminal. `card` says the scene is ABOUT the take-over
// card, which is the one picture where the card is the subject rather than the
// failure; the spinner and the reconnect box are refused there too.
async function expectNoCover(page, { card = false } = {}) {
  await settle(() => coverProblem(page, card))
}

async function coverProblem(page, card) {
  const state = await page.evaluate((wordings) => {
    const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
    const shown = (el) => {
      const r = el.getBoundingClientRect()
      if (r.width < 1 || r.height < 1) return false
      const style = getComputedStyle(el)
      return style.visibility !== "hidden" && Number(style.opacity) >= 0.05
    }
    // Scoped to the pane rather than to the whole document: a toast, a tooltip
    // or a menu elsewhere on the page can carry any of these words, and a guard
    // that refuses a perfectly good picture over one is a guard people turn off.
    // The pane's own wrapper is the scope where there is one; where the pane is
    // not mounted at all (a dormant card, a not-found screen, a phone) the
    // document is, because then the thing being looked for IS what replaced it.
    const container = document.querySelector('[data-testid="terminal-container"]')
    const root =
      document.querySelector('[data-testid="terminal-pane"]') ||
      (container && container.closest(".group.relative")) ||
      document.body
    const text = flat(root.textContent)
    const hit = wordings.find(([words]) => text.includes(flat(words))) || null
    const button = (label) =>
      [...root.querySelectorAll("button")].some((b) => flat(b.textContent) === label && shown(b))
    // The provider spinner names the provider, so it is a shape rather than a
    // fixed phrase.
    const starting = /\bStarting [^…]{1,40}…/.test(text)
    return {
      hit,
      starting,
      takeOver: button("Take over"),
      // A tab with no process behind it. Both surfaces that say so carry this
      // one button, which is also the only way out of them.
      dormant: button("Start session"),
      // The truthful screen for an address naming an agent that is gone. It is
      // a correct render of a wrong scene: every guard about a pane passes
      // because there is no pane.
      notFound: text.includes("Agent not found"),
    }
  }, COVER_WORDINGS)
  if (state.hit) return `${state.hit[1]} ("${state.hit[0]}" is on the pane)`
  if (state.starting) return "the provider is still starting; nothing has painted the pane yet"
  if (state.notFound) {
    return "the pane is the not-found screen; this scene is addressing an agent that is not in the workspace"
  }
  if (state.dormant) {
    return "the tab is dormant and the pane is its start-session card, not a session"
  }
  if (state.takeOver && !card) {
    return "the take-over card is covering the terminal; this scene never took the pty"
  }
  if (!state.takeOver && card) {
    return "this scene is about the take-over card and there is no card on the pane"
  }
  return null
}

// The terminal has something on it, measured off the picture rather than
// inferred from the socket: a pane that attached and then painted nothing is
// exactly what a screenshot must not carry.
//
// The reading is the fraction of pixels that differ from the commonest colour,
// which for a terminal is its own background. A session that just came up paints
// a couple of percent of them; a blank pane paints none.
const PANE_INK_FLOOR = 0.004

async function expectPanePainted(page, { floor = PANE_INK_FLOOR } = {}) {
  await settle(() => paneProblem(page, floor))
}

async function paneProblem(page, floor) {
  const rect = await page.evaluate(() => {
    const el = document.querySelector('[data-testid="terminal-container"]')
    if (!el) return null
    const r = el.getBoundingClientRect()
    if (r.width < 8 || r.height < 8) return null
    return {
      x: Math.max(0, Math.round(r.x)),
      y: Math.max(0, Math.round(r.y)),
      width: Math.round(r.width),
      height: Math.round(r.height),
    }
  })
  if (!rect) return "there is no terminal on this page for the pane guard to read"
  // Wrapped rather than used as handed over: puppeteer answers with a plain
  // Uint8Array, which has none of Buffer's readers.
  const image = decodePng(Buffer.from(await page.screenshot({ clip: rect })))
  const { ratio, colors } = inkRatio(image)
  if (ratio >= floor) return null
  return `the terminal is blank: ${(ratio * 100).toFixed(3)}% of its pixels differ from the background (${colors} colours), under the ${(floor * 100).toFixed(3)}% floor`
}

// --- The artifact ----------------------------------------------------------

// The picture itself, once it exists. Every other guard reads the live page,
// and what gets kept is a separate thing: a pure black frame was written and
// reported as a success with every guard passing twice over, because nothing
// ever looked at the file. This is the last word before it is kept, and it is
// deliberately a weak claim, that the picture is not empty.
function expectCapturePainted(png, options) {
  const problem = captureProblem(png, options)
  if (problem) refuse(problem)
}

// --- The sidebar -----------------------------------------------------------

// One entry per agent row on screen, in document order. Keyed on the row's own
// ⋯ trigger, which every agent row has and no other row does, and read off the
// wrapper that carries the row's highlight.
async function agentRowTexts(page) {
  return page.evaluate(() => {
    const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
    return [...document.querySelectorAll('[aria-label="Session actions"]')]
      .map((trigger) => {
        let row = trigger
        while (row && !row.classList.contains("group/flat-row")) row = row.parentElement
        return row
      })
      .filter((row) => row && row.getBoundingClientRect().height > 1)
      .map((row) => flat(row.textContent))
  })
}

// The agent rows the caption counts, in the order it counts them. Order is part
// of the picture: the staging pins one deliberately, and a sidebar that came
// back sorted some other way is a different screenshot whatever rows it holds.
async function expectRows(page, names) {
  await settle(async () => {
    const rows = await agentRowTexts(page)
    const shown = rows.length ? rows.map((row) => JSON.stringify(row)).join(" | ") : "no agent rows at all"
    if (rows.length !== names.length) {
      return `the sidebar shows ${rows.length} agent rows and the picture is of ${names.length}: ${shown}`
    }
    const wrong = names.findIndex((name, at) => !rows[at].includes(name))
    if (wrong !== -1) {
      return `the sidebar's row ${wrong + 1} is not ${names[wrong]}; the rows read ${shown}`
    }
    return null
  })
}

// The state word on one agent's row, which is what every scene about state is
// actually about.
async function expectStateWord(page, name, word) {
  await settle(async () => {
    const rows = await agentRowTexts(page)
    const row = rows.find((text) => text.includes(name))
    if (!row) return `there is no ${name} row on this page to read a state word off`
    if (row.includes(word)) return null
    return `the ${name} row does not read "${word}"; it reads ${JSON.stringify(row)}`
  })
}

// --- Menus and dialogs -----------------------------------------------------

// A menu is open, and it is the right one, named by an item only that menu has.
async function expectMenuOpen(page, itemText) {
  await settle(async () => {
    const menus = await page.evaluate(() => {
      const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
      return [...document.querySelectorAll('[role="menu"]')]
        .filter((el) => el.getBoundingClientRect().height > 1)
        .map((el) => flat(el.textContent))
    })
    if (!menus.length) return `no menu is open; this scene is a picture of one offering "${itemText}"`
    if (menus.some((text) => text.includes(flatten(itemText)))) return null
    return `the open menu does not offer "${itemText}": ${menus.map((text) => JSON.stringify(text)).join(" | ")}`
  })
}

// A dialog is open, and it is the right one, named by its title.
async function expectDialogOpen(page, title) {
  await settle(async () => {
    const dialogs = await page.evaluate(() => {
      const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
      return [...document.querySelectorAll('[role="dialog"]')]
        .filter((el) => el.getBoundingClientRect().height > 1)
        .map((el) => flat(el.textContent).slice(0, 200))
    })
    if (!dialogs.length) return `no dialog is open; this scene is a picture of "${title}"`
    if (dialogs.some((text) => text.includes(flatten(title)))) return null
    return `the open dialog is not "${title}": ${dialogs.map((text) => JSON.stringify(text)).join(" | ")}`
  })
}

module.exports = {
  ATTENTION_AGENT,
  ATTENTION_WORD,
  BASE,
  SIDEBAR_ORDER,
  armAttention,
  openMoreWays,
  openNewAgent,
  containerSh,
  stage,
  DESKTOP,
  PHONE,
  agents,
  api,
  boxOf,
  clearToasts,
  clickLabel,
  clickText,
  // Every guard goes out recorded, so a scene's questions are asked again with
  // the shutter open rather than only where the scene wrote them.
  // Not recorded: it reads the artifact rather than the page, and it runs once,
  // when there is an artifact to read.
  expectCapturePainted,
  expectBanner: recorded(expectBanner),
  expectDialogOpen: recorded(expectDialogOpen),
  expectFieldValue: recorded(expectFieldValue),
  expectMenuOpen: recorded(expectMenuOpen),
  expectNoCover: recorded(expectNoCover),
  expectPanePainted: recorded(expectPanePainted),
  expectRows: recorded(expectRows),
  expectStateWord: recorded(expectStateWord),
  expectVisible: recorded(expectVisible),
  expectVisibleText: recorded(expectVisibleText),
  recheckGuards,
  freshen,
  get,
  goto,
  isRefusal,
  measureInk,
  refusal,
  open,
  project,
  recordedGuardCount,
  setCaptureClip,
  setFixture,
  setSceneName,
  sleep,
  takeOver,
}
