// A dock that appears under an agent already on screen, or on the agent the
// page moves to, used to replace the centre pane with a "dux needs a reload"
// card and log "Invalid 1 panel layout". The terminal is added through the
// API, as another device or the command line would add it.

const WITHOUT = "polish-onboarding"
const WITH = "add-rate-limits"

// With a single terminal the dock's strip stays hidden, so the dock is told by
// its terminal: a second terminal box on screen beside the agent's.
const dockShown = () =>
  [...document.querySelectorAll('[data-testid="terminal-container"]')].filter((el) => el.getBoundingClientRect().height > 0).length >= 2

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
    "On a computer, moving from an agent with no terminal to one with a terminal docked under it, and opening the first terminal on the agent on screen, each show the agent with its dock and no error card.",
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

    const dev = await H.device(ctx, "computer")
    const logged = []
    dev.page.on("console", (m) => {
      if (m.type() === "error" && /panel layout/i.test(m.text())) logged.push(m.text())
    })
    // A production build reports a caught render error as a page error.
    dev.page.on("pageerror", (e) => {
      if (/panel layout/i.test(e.message)) logged.push(e.message)
    })
    await H.goto(dev, H.agentHash(plain))
    await H.drive(dev)

    // After each step the pane must show a dock and no error card, and the
    // console must carry no complaint from the split.
    const settled = async (what) => {
      // The wait is for the dock to arrive; whether it did is judged below.
      await H.until(async () => dev.page.evaluate(dockShown), `the dock under ${what}`, { timeout: 8000 }).catch(() => {})
      const card = await dev.page.evaluate(() => /needs a reload|hit an error/.test(document.body.innerText))
      H.expect(!card, `${what} should show no error card; the pane shows one`)
      H.expect(await dev.page.evaluate(dockShown), `${what} should show its dock under the agent; only one terminal box is on screen`)
      H.expect(logged.length === 0, `${what} should log no split-pane layout error; the console said "${logged[0]}"`)
    }

    // Moving to an agent whose dock is there from the start, with no reload.
    await clickRow(dev, WITH, H.expect)
    await settled(`${WITH} after ${WITHOUT}`)

    // Back on the agent without a terminal, then giving it its first one.
    await dev.page.evaluate((h) => { location.hash = h }, H.agentHash(plain))
    await H.until(
      async () => !(await dev.page.evaluate(dockShown)),
      `${WITHOUT} to show no dock`,
    )
    await H.lib.api("POST", `/api/v1/sessions/${plain}/terminals`)
    await settled(`${WITHOUT} once its first terminal opens`)
  },
}
