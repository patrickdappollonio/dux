// An agent with one tab and one terminal and the strips hidden: the divider is
// a thin grip band and the layout buttons float in a pill on its right end.
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
  file: "web-terminal-dock-hidden-strips.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    const by = await agents()
    const id = by["fix-login-redirect"].id
    after(await dockTerminals(id, 1))
    await goto(page, `#/agent/${id}`)
    await takeOver(page)
    await sleep(2500)
    await clearToasts(page)
    await expectNoCover(page)
    await expectVisible(page, '[data-testid="stack-layout-pill"]', "the floating layout pill")
    await expectVisible(page, '[role="separator"][aria-orientation="horizontal"]', "the grip band")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
