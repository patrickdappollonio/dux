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
    const frames = []
    dev.cdp.on("Network.webSocketCreated", (e) => {
      if (/\/pty\b/.test(e.url)) frames.socket = e.requestId
    })
    dev.cdp.on("Network.webSocketFrameSent", (e) => {
      if (e.requestId === frames.socket) frames.push(e.response.payloadData)
    })
    await H.goto(dev, H.agentHash(id))
    await H.drive(dev)
    const opened = await openPicker(H, dev)
    H.expect(opened.rows.length >= 2, `the macro picker should list the seeded macros; it lists ${JSON.stringify(opened.rows)}`)
    H.expect(opened.rowButtons.length === 0, `the picker's rows should carry no button of their own; these do: ${opened.rowButtons.join(", ")}`)
    await filterTo(H, dev)
    frames.length = 0
    await dev.page.keyboard.press("Enter")
    const decoded = () => frames.map((f) => Buffer.from(f, "base64").toString("utf8")).join("")
    await H.until(() => decoded().includes(TEXT), `"${MACRO}" to be sent`, { timeout: 4000 }).catch(() =>
      H.fail(`Enter on "${MACRO}" should send "${TEXT}" down the pane's socket; it sent ${JSON.stringify(decoded().slice(0, 200))}`),
    )

    await openPicker(H, dev)
    await H.press(dev, { name: "^Edit macros…$", selector: "button" })
    await H.waitForDialog(dev, "^Macros$")

    const phone = await H.device(ctx, "phone")
    await H.goto(phone, H.agentHash(id))
    await H.drive(phone)
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
