// A phone pane screen: the header keeps navigation, identity and the PR chip,
// and the pane's actions hang off the band as a flap, the agent/terminals
// switch among them because the agent has a terminal of its own.
const {
  agents,
  clearToasts,
  dockTerminals,
  expectNoCover,
  expectPanePainted,
  expectVisible,
  expectVisibleText,
  goto,
  showAgent,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "phone-terminal-flap.png",
  viewport: "phone",
  async shoot(page, { after }) {
    const by = await agents()
    const id = by["fix-login-redirect"].id
    after(await dockTerminals(id, 1))
    await goto(page, `#/agent/${id}`)
    await takeOver(page)
    await sleep(1000)
    await showAgent(page)
    await sleep(1500)
    await clearToasts(page)
    // The flap the caption is about, the identity the slim header keeps, and a
    // terminal underneath that came up.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisible(page, '[data-testid="mobile-action-flap"]', "the pane's action flap")
    await expectVisible(page, '[aria-label^="Show terminals (1)"]', "the switch to the terminals")
    await expectVisibleText(page, "fix-login-redirect", { what: "the agent the header names" })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}

