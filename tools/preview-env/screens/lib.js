// Shared plumbing for the committed screenshot scenes: one browser contract and
// one REST client, so a scene file holds only what makes its own picture.
//
// The launch contract is shot.js's, for the same measured reasons: the scale is
// a browser flag, the CDP metrics override is sent with deviceScaleFactor 0, and
// SwiftShader is asked for by name so the terminal's webgl canvas is not
// captured black.
const { spawnSync } = require("child_process")
const path = require("path")
const zlib = require("zlib")

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
  // pixels rather than the emulated viewport's, so the override is re-asserted
  // (idempotently) immediately before every capture.
  const reassert = () => cdp.send("Emulation.setDeviceMetricsOverride", metrics)
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
// without every call site repeating its own name.
let sceneUnderShot = "scene"
const setSceneName = (name) => {
  sceneUnderShot = name
}

function refuse(what) {
  throw new Error(`${sceneUnderShot}: ${what}`)
}

// Whitespace is normalized on both sides: the DOM's own line breaks and the
// spaces the UI puts between a glyph and its word are not differences a caption
// is about.
const flatten = (text) => (text || "").replace(/\s+/g, " ").trim()

// The smallest visible element carrying a phrase, with its width, or null. The
// smallest one is the one that OWNS the text: every ancestor up to <body>
// contains it too, and their widths say nothing.
async function findText(page, text, selector) {
  return page.evaluate(
    (sel, needle) => {
      const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
      const want = flat(needle)
      let best = null
      for (const el of document.querySelectorAll(sel)) {
        if (!flat(el.textContent).includes(want)) continue
        const r = el.getBoundingClientRect()
        if (r.width < 1 || r.height < 1) continue
        const style = getComputedStyle(el)
        if (style.visibility === "hidden" || Number(style.opacity) < 0.05) continue
        const area = r.width * r.height
        if (!best || area < best.area) best = { area, width: r.width, height: r.height }
      }
      return best
    },
    selector,
    text,
  )
}

// A phrase the caption promises, visible somewhere on the page.
async function expectVisibleText(page, text, { selector = "*", what } = {}) {
  const found = await findText(page, text, selector)
  if (!found) refuse(`${what || `the words "${flatten(text)}"`} are not on the page`)
}

// A visible element, named by what it is rather than by its selector: a scene
// whose subject is a shape (a flap, a pill, an overlay) has no words of its own
// to look for.
async function expectVisible(page, selector, what) {
  const there = await page.evaluate(
    (sel) =>
      [...document.querySelectorAll(sel)].some((el) => {
        const r = el.getBoundingClientRect()
        if (r.width < 1 || r.height < 1) return false
        const style = getComputedStyle(el)
        return style.visibility !== "hidden" && Number(style.opacity) >= 0.05
      }),
    selector,
  )
  if (!there) refuse(`${what} is not on screen (nothing visible matches ${selector})`)
}

// A wide strip of chrome rather than an incidental mention of the same words:
// the sidebar row's pull-request chip carries the banner's own text at chip
// width, and a picture of the chip is not a picture of the banner.
async function expectBanner(page, text, { minWidth = 300, what } = {}) {
  const found = await findText(page, text, "*")
  if (!found) refuse(`${what || `the banner reading "${flatten(text)}"`} is not on the page`)
  if (found.width < minWidth) {
    refuse(
      `${what || `"${flatten(text)}"`} is only ${Math.round(found.width)}px wide, which is a chip rather than a banner`,
    )
  }
}

// What a field holds, which is its value rather than anything in the DOM.
async function expectFieldValue(page, selector, text) {
  const values = await page.evaluate(
    (sel) => [...document.querySelectorAll(sel)].map((el) => el.value || ""),
    selector,
  )
  if (!values.some((value) => flatten(value).includes(flatten(text)))) {
    refuse(
      `no ${selector} holds "${flatten(text)}"; they hold ${JSON.stringify(values.map(flatten))}`,
    )
  }
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
  const state = await page.evaluate((wordings) => {
    const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
    const shown = (el) => {
      const r = el.getBoundingClientRect()
      if (r.width < 1 || r.height < 1) return false
      const style = getComputedStyle(el)
      return style.visibility !== "hidden" && Number(style.opacity) >= 0.05
    }
    const body = flat(document.body.textContent)
    const hit = wordings.find(([words]) => body.includes(flat(words))) || null
    const takeOver = [...document.querySelectorAll("button")].some(
      (b) => flat(b.textContent) === "Take over" && shown(b),
    )
    // The provider spinner names the provider, so it is a shape rather than a
    // fixed phrase.
    const starting = /\bStarting [^…]{1,40}…/.test(body)
    return { hit, takeOver, starting }
  }, COVER_WORDINGS)
  if (state.hit) refuse(`${state.hit[1]} ("${state.hit[0]}" is on the pane)`)
  if (state.starting) refuse("the provider is still starting; nothing has painted the pane yet")
  if (state.takeOver && !card) {
    refuse("the take-over card is covering the terminal; this scene never took the pty")
  }
  if (!state.takeOver && card) {
    refuse("this scene is about the take-over card and there is no card on the pane")
  }
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
  if (!rect) refuse("there is no terminal on this page for the pane guard to read")
  // Wrapped rather than used as handed over: puppeteer answers with a plain
  // Uint8Array, which has none of Buffer's readers.
  const image = decodePng(Buffer.from(await page.screenshot({ clip: rect })))
  const { ratio, colors } = inkRatio(image)
  if (ratio < floor) {
    refuse(
      `the terminal is blank: ${(ratio * 100).toFixed(3)}% of its pixels differ from the background (${colors} colours), under the ${(floor * 100).toFixed(3)}% floor`,
    )
  }
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

// The agent rows the caption counts, and no others.
async function expectRows(page, names) {
  const rows = await agentRowTexts(page)
  const missing = names.filter((name) => !rows.some((row) => row.includes(name)))
  if (missing.length) {
    refuse(
      `the sidebar is missing ${missing.join(", ")}; it shows ${rows.length ? rows.map((row) => JSON.stringify(row)).join(" | ") : "no agent rows at all"}`,
    )
  }
  if (rows.length !== names.length) {
    refuse(
      `the sidebar shows ${rows.length} agent rows and the picture is of ${names.length}: ${rows.map((row) => JSON.stringify(row)).join(" | ")}`,
    )
  }
}

// The state word on one agent's row, which is what every scene about state is
// actually about.
async function expectStateWord(page, name, word) {
  const rows = await agentRowTexts(page)
  const row = rows.find((text) => text.includes(name))
  if (!row) refuse(`there is no ${name} row on this page to read a state word off`)
  if (!row.includes(word)) {
    refuse(`the ${name} row does not read "${word}"; it reads ${JSON.stringify(row)}`)
  }
}

// --- Menus and dialogs -----------------------------------------------------

// A menu is open, and it is the right one, named by an item only that menu has.
async function expectMenuOpen(page, itemText) {
  const menus = await page.evaluate(() => {
    const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
    return [...document.querySelectorAll('[role="menu"]')]
      .filter((el) => el.getBoundingClientRect().height > 1)
      .map((el) => flat(el.textContent))
  })
  if (!menus.length) {
    refuse(`no menu is open; this scene is a picture of one offering "${itemText}"`)
  }
  if (!menus.some((text) => text.includes(flatten(itemText)))) {
    refuse(
      `the open menu does not offer "${itemText}": ${menus.map((text) => JSON.stringify(text)).join(" | ")}`,
    )
  }
}

// A dialog is open, and it is the right one, named by its title.
async function expectDialogOpen(page, title) {
  const dialogs = await page.evaluate(() => {
    const flat = (value) => (value || "").replace(/\s+/g, " ").trim()
    return [...document.querySelectorAll('[role="dialog"]')]
      .filter((el) => el.getBoundingClientRect().height > 1)
      .map((el) => flat(el.textContent).slice(0, 200))
  })
  if (!dialogs.length) refuse(`no dialog is open; this scene is a picture of "${title}"`)
  if (!dialogs.some((text) => text.includes(flatten(title)))) {
    refuse(
      `the open dialog is not "${title}": ${dialogs.map((text) => JSON.stringify(text)).join(" | ")}`,
    )
  }
}

// --- Reading a capture -----------------------------------------------------

// Enough of a PNG reader to measure one. Chromium writes 8-bit, non-interlaced
// RGB or RGBA, and a screenshot is the only picture this ever looks at; a
// dependency for it would have to be installed before a scene could even be
// loaded, and the website's loop test loads every scene with nothing installed.
function decodePng(buffer) {
  if (buffer.readUInt32BE(0) !== 0x89504e47) throw new Error("the capture is not a PNG")
  let offset = 8
  let width = 0
  let height = 0
  let depth = 0
  let colorType = 0
  const parts = []
  while (offset + 8 <= buffer.length) {
    const length = buffer.readUInt32BE(offset)
    const type = buffer.toString("ascii", offset + 4, offset + 8)
    const body = buffer.subarray(offset + 8, offset + 8 + length)
    if (type === "IHDR") {
      width = body.readUInt32BE(0)
      height = body.readUInt32BE(4)
      depth = body[8]
      colorType = body[9]
      if (body[12] !== 0) throw new Error("the capture is interlaced")
    } else if (type === "IDAT") {
      parts.push(body)
    } else if (type === "IEND") {
      break
    }
    offset += length + 12
  }
  if (depth !== 8 || (colorType !== 2 && colorType !== 6)) {
    throw new Error(`unsupported capture (bit depth ${depth}, colour type ${colorType})`)
  }
  const channels = colorType === 6 ? 4 : 3
  const raw = zlib.inflateSync(Buffer.concat(parts))
  const stride = width * channels
  const pixels = Buffer.alloc(height * stride)
  let previous = Buffer.alloc(stride)
  for (let y = 0; y < height; y++) {
    const at = y * (stride + 1)
    const filter = raw[at]
    const line = raw.subarray(at + 1, at + 1 + stride)
    const out = pixels.subarray(y * stride, (y + 1) * stride)
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? out[i - channels] : 0
      const b = previous[i]
      const c = i >= channels ? previous[i - channels] : 0
      let value
      switch (filter) {
        case 0:
          value = line[i]
          break
        case 1:
          value = line[i] + a
          break
        case 2:
          value = line[i] + b
          break
        case 3:
          value = line[i] + ((a + b) >> 1)
          break
        case 4: {
          const p = a + b - c
          const pa = Math.abs(p - a)
          const pb = Math.abs(p - b)
          const pc = Math.abs(p - c)
          value = line[i] + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c)
          break
        }
        default:
          throw new Error(`unknown PNG filter ${filter}`)
      }
      out[i] = value & 0xff
    }
    previous = out
  }
  return { width, height, channels, pixels }
}

// How much of an image is not its own background. Every other pixel in each
// direction is sampled, which is four times less arithmetic and cannot miss a
// glyph: text is many pixels wide at this scale.
const INK_DISTANCE = 24

// The same reading `expectPanePainted` refuses on, off any PNG. Exported so the
// floor above can be argued with a number rather than adjusted by feel.
const measureInk = (png) => inkRatio(decodePng(png))

function inkRatio(image) {
  const { width, height, channels, pixels } = image
  const counts = new Map()
  const sample = []
  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      const i = y * width * channels + x * channels
      const key = (pixels[i] << 16) | (pixels[i + 1] << 8) | pixels[i + 2]
      counts.set(key, (counts.get(key) || 0) + 1)
      sample.push(key)
    }
  }
  let background = 0
  let most = -1
  for (const [key, n] of counts) {
    if (n > most) {
      most = n
      background = key
    }
  }
  const br = background >> 16
  const bg = (background >> 8) & 0xff
  const bb = background & 0xff
  let ink = 0
  for (const key of sample) {
    const dr = Math.abs((key >> 16) - br)
    const dg = Math.abs(((key >> 8) & 0xff) - bg)
    const db = Math.abs((key & 0xff) - bb)
    if (Math.max(dr, dg, db) > INK_DISTANCE) ink++
  }
  return { ratio: sample.length ? ink / sample.length : 0, colors: counts.size }
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
  expectBanner,
  expectDialogOpen,
  expectFieldValue,
  expectMenuOpen,
  expectNoCover,
  expectPanePainted,
  expectRows,
  expectStateWord,
  expectVisible,
  expectVisibleText,
  freshen,
  get,
  goto,
  measureInk,
  open,
  project,
  setFixture,
  setSceneName,
  sleep,
  takeOver,
}
