// An agent with two terminals of its own docked under it: the agent and its tab
// strip on top, the dock's strip and the terminal in front below, the layout
// buttons at the dock strip's right end.
const {
  agents,
  clearToasts,
  dockTerminals,
  expectNoCover,
  expectVisible,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "web-terminal-dock.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    const by = await agents()
    const id = by["fix-login-redirect"].id
    after(await dockTerminals(id, 2))
    await goto(page, `#/agent/${id}`)
    await takeOver(page)
    await sleep(2500)
    await clearToasts(page)
    await expectNoCover(page)
    // Two terminals bring both strips up: the dock's, with its two pills and
    // the layout buttons the caption names.
    await expectVisible(page, '[data-testid="dock-strip"]', "the dock strip")
    await expectVisible(page, '[aria-label="Maximize terminal"]', "the maximize button")
    await expectVisible(page, '[aria-label="Hide terminals"]', "the hide button")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
