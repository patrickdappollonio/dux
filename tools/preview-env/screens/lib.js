// Shared plumbing for the committed screenshot scenes: one browser contract and
// one REST client, so a scene file holds only what makes its own picture.
//
// The launch contract is shot.js's, for the same measured reasons: the scale is
// a browser flag, the CDP metrics override is sent with deviceScaleFactor 0, and
// SwiftShader is asked for by name so the terminal's webgl canvas is not
// captured black.
const { spawnSync } = require("child_process")
const path = require("path")

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

// Refuse to write a picture of the wrong thing: the sidebar row has to say the
// words the caption promises. Same idea as the take-over card's guard.
async function assertNeedsAttention(page, title = ATTENTION_AGENT) {
  const reads = await page.evaluate((name) => {
    // Bounded by length so this matches the row rather than an ancestor that
    // happens to contain every row.
    const row = [...document.querySelectorAll("a, li, div")].find((el) => {
      const text = el.textContent || ""
      return text.length < 200 && text.includes(name) && /Needs you/.test(text)
    })
    return Boolean(row)
  }, title)
  if (!reads) {
    throw new Error(
      `the ${title} row does not read "Needs you"; something cleared the flag before this scene`,
    )
  }
}

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

module.exports = {
  ATTENTION_AGENT,
  BASE,
  SIDEBAR_ORDER,
  armAttention,
  assertNeedsAttention,
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
  freshen,
  get,
  goto,
  open,
  project,
  setFixture,
  sleep,
  takeOver,
}
