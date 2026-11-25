// A dock that appears under an agent already on screen, or on the agent the
// page moves to, must not replace the centre pane with a "dux needs a reload"
// card or log "Invalid 1 panel layout". The terminal is added through the
// API rather than the page's own controls, the first one as the command line
// would and the last as the page driving the row would. The move crashed
// only when the browser had last left the docked agent with its terminal
// folded away or alone on screen (a stored layout of "agent" or "terminal");
// a fresh browser or a stored split never did, so the check seeds that memory.

const WITHOUT = "polish-onboarding"
const WITH = "add-rate-limits"

// With a single terminal the dock's strip stays hidden, so the dock is told by
// its terminal: a second terminal box on screen beside the agent's.
const dockShown = () =>
  [...document.querySelectorAll('[data-testid="terminal-container"]')].filter((el) => el.getBoundingClientRect().height > 0).length >= 2

// The id a page names on its terminal sockets (`?page=`), which a request
// carries in `x-dux-page` to be the page that drives the row. Listening starts
// before the page loads, so its first socket is seen.
async function watchPageSeat(dev) {
  const seats = new Set()
  const cdp = await dev.page.createCDPSession()
  await cdp.send("Network.enable")
  cdp.on("Network.webSocketCreated", ({ url }) => {
    const m = /[?&]page=([A-Za-z0-9_-]+)/.exec(url)
    if (m) seats.add(m[1])
  })
  return seats
}

// A person's click on an agent's name in the sidebar list.
async function clickRow(dev, title, expectRow) {
  const at = await dev.page.evaluate((t) => {
    const el = [...document.querySelectorAll("span,div,p")].find(
      (e) => e.children.length === 0 && (e.textContent || "").trim() === t && e.getBoundingClientRect().x < 300,
    )
    const r = el && el.getBoundingClientRect()
    return r ? { x: r.x + r.width / 2, y: r.y + r.height / 2 } : null
  }, title)
  expectRow(at !== null, `the sidebar should list ${title}; it does not`)
  await dev.page.mouse.click(at.x, at.y)
}

module.exports = {
  protects:
    "On a computer, moving from an agent with no terminal to one whose terminal was last left folded away or alone on screen, and opening the first terminal on the agent on screen, each show the agent with its dock and no error card.",
  async run(ctx, H) {
    const plain = await H.agentId(WITHOUT)
    const docked = await H.agentId(WITH)

    // Registered before anything is added, so an interrupt still closes them.
    ctx.after(async () => {
      const terminals = (await H.lib.get("/api/v1/workspace")).terminals || []
      for (const t of terminals) {
        const id = t.owner.kind === "session" ? t.owner.session_id : null
        if (id !== plain && id !== docked) continue
        await H.lib.api("DELETE", `/api/v1/sessions/${id}/terminals/${t.id}?force_connected=true`).catch(() => {})
      }
    })
    await H.lib.api("POST", `/api/v1/sessions/${docked}/terminals`)

    // The memory a browser keeps of how it last left the docked agent, set
    // before the app loads (the devices are fresh browser contexts, so nothing
    // needs undoing). "agent" and "terminal" are the layouts that crashed.
    const leftAs = async (layout) => {
      const dev = await H.device(ctx, "computer")
      await dev.page.evaluateOnNewDocument(
        (id, l) => {
          try {
            localStorage.setItem(`dux:stack:${id}`, JSON.stringify({ layout: l, view: null, terminal: null, tab: null }))
          } catch {
            // A frame with no storage has nothing to seed.
          }
        },
        docked,
        layout,
      )
      return dev
    }

    const watch = (dev) => {
      const logged = []
      dev.page.on("console", (m) => {
        if (m.type() === "error" && /panel layout/i.test(m.text())) logged.push(m.text())
      })
      // A production build reports a caught render error as a page error.
      dev.page.on("pageerror", (e) => {
        if (/panel layout/i.test(e.message)) logged.push(e.message)
      })
      return logged
    }

    // After each step the pane must show a dock and no error card, and the
    // console must carry no complaint from the split.
    const settled = async (dev, logged, what) => {
      // The wait is for the dock to arrive; whether it did is judged below.
      await H.until(async () => dev.page.evaluate(dockShown), `the dock under ${what}`, { timeout: 8000 }).catch(() => {})
      const card = await dev.page.evaluate(() => /needs a reload|hit an error/.test(document.body.innerText))
      H.expect(!card, `${what} should show no error card; the pane shows one`)
      H.expect(await dev.page.evaluate(dockShown), `${what} should show its dock under the agent; only one terminal box is on screen`)
      H.expect(logged.length === 0, `${what} should log no split-pane layout error; the console said "${logged[0]}"`)
    }

    // Moving to an agent whose dock is there from the start, with no reload.
    // Each of these devices claimed both rows on its way, so each is closed
    // once it has been judged, leaving the last device the only one there.
    for (const layout of ["agent", "terminal"]) {
      const dev = await leftAs(layout)
      const logged = watch(dev)
      await H.goto(dev, H.agentHash(plain))
      await H.drive(dev)
      await clickRow(dev, WITH, H.expect)
      await settled(dev, logged, `${WITH} (last left in the ${layout} layout) after ${WITHOUT}`)
      await dev.context.close()
    }
    await H.until(async () => {
      const sessions = await H.lib.get("/api/v1/sessions")
      return !sessions.some((s) => (s.id === plain || s.id === docked) && s.tabs.some((t) => t.input_owner))
    }, "the closed devices to let go of both agents' rows", { timeout: 8000 })

    // Back on the agent without a terminal, then giving it its first one, as
    // the page that drives it: nobody else holds the row, so nothing is forced.
    const dev = await H.device(ctx, "computer")
    const logged = watch(dev)
    const seats = await watchPageSeat(dev)
    await H.goto(dev, H.agentHash(plain))
    await H.drive(dev)
    H.expect(seats.size === 1, `the page should name one id on its terminal sockets; it named ${seats.size}`)
    const res = await fetch(`${H.lib.BASE}/api/v1/sessions/${plain}/terminals`, {
      method: "POST",
      headers: { "x-dux-page": [...seats][0] },
    })
    H.expect(res.ok, `the page driving ${WITHOUT} should be let open its first terminal; the server answered ${res.status} ${(await res.text()).slice(0, 200)}`)
    await settled(dev, logged, `${WITHOUT} once its first terminal opens`)
  },
}
