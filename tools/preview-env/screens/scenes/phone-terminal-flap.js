// A phone pane screen: the header keeps navigation, identity and the PR chip,
// and the pane's actions hang off the band as a flap.
const {
  agents,
  clearToasts,
  expectNoCover,
  expectPanePainted,
  expectVisible,
  expectVisibleText,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "phone-terminal-flap.png",
  viewport: "phone",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await sleep(2000)
    await clearToasts(page)
    // The flap the caption is about, the identity the slim header keeps, and a
    // terminal underneath that came up.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisible(page, '[data-testid="mobile-action-flap"]', "the pane's action flap")
    await expectVisibleText(page, "fix-login-redirect", { what: "the agent the header names" })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
