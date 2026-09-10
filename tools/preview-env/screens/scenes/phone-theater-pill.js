// Theater mode on a phone: the top chrome is gone and the floating pill is the
// way back.
const { agents, clearToasts, goto, sleep, takeOver } = require("../lib.js")

module.exports = {
  file: "phone-theater-pill.png",
  viewport: "phone",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await sleep(1500)
    // Not a pointer click: entering theater runs the flap-into-pill flight, and
    // a CDP mouse dispatch waits on it long enough to time the protocol out.
    await page.evaluate(() => {
      const el =
        document.querySelector('[data-testid="pane-theater-toggle"]') ||
        [...document.querySelectorAll('[aria-label="Theater mode"]')].find(
          (e) => e.getBoundingClientRect().width > 0,
        )
      el.click()
    })
    await sleep(3500)
    // The click leaves the pill's toggle focused, which paints a focus ring and
    // opens its tooltip; neither belongs in a screenshot of the mode.
    await page.evaluate(() => document.activeElement?.blur())
    await sleep(1200)
    await clearToasts(page)
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
