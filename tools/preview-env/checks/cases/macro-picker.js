// The macro picker is a list to pick from: its rows carry no buttons of their
// own, typing filters them, Enter sends the highlighted one where the pane
// sends typing, and its footer opens the Macros editor.
const { AGENT } = require("../dialogs.js")

const QUERY = "lint"
const MACRO = "Lint"
const TEXT = "Run the linter and fix what it reports."

async function picker(page) {
  return page.evaluate(() => {
    const search = [...document.querySelectorAll('input[role="combobox"]')].find((i) => /macro/i.test(i.placeholder || ""))
    const popup = search && search.closest('[role="dialog"]')
    if (!popup) return null
    const rows = [...popup.querySelectorAll('[role="option"]')].filter((o) => o.getBoundingClientRect().height > 0)
    return {
      rows: rows.map((o) => window.__checkNameOf(o)),
      rowButtons: rows.filter((o) => o.querySelector("button,[role=button],a[href]")).map((o) => window.__checkNameOf(o)),
      highlighted: rows.filter((o) => o.getAttribute("aria-selected") === "true" || o.hasAttribute("data-highlighted")).map((o) => window.__checkNameOf(o)),
    }
  })
}

async function openPicker(H, dev) {
  await H.press(dev, { name: "^Run a macro$", selector: "button" })
  return H.until(() => picker(dev.page), "the macro picker to open")
}

async function filterTo(H, dev) {
  await dev.page.keyboard.type(QUERY)
  const filtered = await H.until(async () => {
    const now = await picker(dev.page)
    return now && now.rows.length === 1 ? now : null
  }, `typing "${QUERY}" to filter the picker`, { timeout: 3000 }).catch(async () =>
    H.fail(`typing "${QUERY}" should leave only "${MACRO}" in the picker; it lists ${JSON.stringify((await picker(dev.page) || {}).rows)}`),
  )
  H.expect(filtered.rows[0] === MACRO, `typing "${QUERY}" should leave "${MACRO}"; it left "${filtered.rows[0]}"`)
  H.expect(filtered.highlighted[0] === MACRO, `the one row left, "${MACRO}", should be highlighted for Enter; highlighted: ${JSON.stringify(filtered.highlighted)}`)
}

module.exports = {
  protects:
    "The macro picker's rows carry no per-row button, typing filters them, Enter runs the highlighted macro (its bytes go out on the pane's socket on a computer, its text lands in the message box on a phone), and \"Edit macros…\" opens the Macros dialog.",
  async run(ctx, H) {
    const id = await H.agentId(AGENT)

    const dev = await H.device(ctx, "computer")
    await dev.cdp.send("Network.enable")
    // What the page sends down its terminal sockets. Keystrokes travel as binary
    // frames (base64 in what the browser reports); text frames are the socket's
    // own JSON (a resize, a beat) and carry no typing.
    const sockets = new Set()
    const sent = []
    dev.cdp.on("Network.webSocketCreated", (e) => {
      if (/\/pty(\?|$)/.test(new URL(e.url).pathname + "?")) sockets.add(e.requestId)
    })
    dev.cdp.on("Network.webSocketFrameSent", (e) => {
      if (sockets.has(e.requestId)) sent.push({ binary: e.response.opcode === 2, data: e.response.payloadData })
    })
    await H.goto(dev, H.agentHash(id))
    await H.drive(dev)
    const opened = await openPicker(H, dev)
    H.expect(opened.rows.length >= 2, `the macro picker should list the seeded macros; it lists ${JSON.stringify(opened.rows)}`)
    H.expect(opened.rowButtons.length === 0, `the picker's rows should carry no button of their own; these do: ${opened.rowButtons.join(", ")}`)
    await filterTo(H, dev)
    sent.length = 0
    await dev.page.keyboard.press("Enter")
    const typed = () =>
      sent
        .filter((f) => f.binary)
        .map((f) => Buffer.from(f.data, "base64").toString("utf8"))
        .join("")
    await H.until(() => typed().includes(TEXT), `"${MACRO}" to be sent`, { timeout: 4000 }).catch(() =>
      H.fail(
        `Enter on "${MACRO}" should type "${TEXT}" down the pane's socket; it typed ${JSON.stringify(typed().slice(0, 200))}${sent.some((f) => !f.binary) ? ` and sent the text frames ${JSON.stringify(sent.filter((f) => !f.binary).map((f) => f.data.slice(0, 80)))}` : ""}`,
      ),
    )

    await openPicker(H, dev)
    await H.press(dev, { name: "^Edit macros…$", selector: "button" })
    await H.waitForDialog(dev, "^Macros$")

    // The computer still drives the agent, so the phone takes it over.
    const phone = await H.device(ctx, "phone")
    await H.goto(phone, H.agentHash(id))
    await H.takeOver(phone)
    await openPicker(H, phone)
    await H.press(phone, { name: "^Search macros", selector: 'input[role="combobox"]' })
    await filterTo(H, phone)
    await phone.page.keyboard.press("Enter")
    await H.waitFor(
      phone.page,
      (text) => [...document.querySelectorAll("textarea")].some((t) => t.value.includes(text)),
      `"${MACRO}" to land in the message box`,
      { arg: TEXT, timeout: 4000 },
    ).catch(async () => {
      const boxes = await phone.page.evaluate(() => [...document.querySelectorAll("textarea")].map((t) => t.value))
      H.fail(`on a phone Enter on "${MACRO}" should put "${TEXT}" in the message box; the text boxes hold ${JSON.stringify(boxes)}`)
    })
  },
}
