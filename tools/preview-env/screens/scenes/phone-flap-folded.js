// The same phone pane screen as phone-terminal-flap, with the action flap folded
// up into the band by the button at the right end of the header, so the
// terminal's first lines read. The header keeps Back, the agent's identity, the
// PR chip and the button that brings the flap back.
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
  file: "phone-flap-folded.png",
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
    await page.evaluate(() => {
      const fold = document.querySelector('[aria-label="Hide actions"]')
      if (fold) fold.click()
    })
    // The fold slides; the picture is of where it comes to rest.
    await sleep(1500)
    // The header's button now offers the flap back, the header still names the
    // agent, and the terminal under where the flap hung is painted.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisible(page, '[aria-label="Show actions"]', "the header button that shows the flap again")
    await expectVisibleText(page, "fix-login-redirect", { what: "the agent the header names" })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
