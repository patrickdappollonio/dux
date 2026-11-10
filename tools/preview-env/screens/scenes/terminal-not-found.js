// A link to one of an agent's terminals that has since closed: the center pane
// says the terminal is not found, names its id, and offers a new terminal for
// the same agent.
const {
  agents,
  clearToasts,
  dockTerminals,
  expectVisibleText,
  get,
  goto,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "terminal-not-found.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    const id = by["fix-login-redirect"].id
    const before = await ownTerminals(id)
    const close = await dockTerminals(id, before.length + 1)
    const closed = (await ownTerminals(id)).find((t) => !before.includes(t))
    if (!closed) throw new Error("no terminal was opened to close")
    await close()
    await sleep(800)
    await goto(page, `#/agent/${id}/terminal/${closed}`)
    await sleep(1500)
    await clearToasts(page)
    await expectVisibleText(page, "Terminal not found", { what: "the not-found title" })
    await expectVisibleText(page, "New terminal", { selector: "button", what: "the New terminal button" })
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}

async function ownTerminals(sessionId) {
  return ((await get("/api/v1/workspace")).terminals || [])
    .filter((t) => t.owner.kind === "session" && t.owner.session_id === sessionId)
    .map((t) => t.id)
}
