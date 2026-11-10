// Theater mode on a phone: the top chrome is gone and the floating pill is the
// way back, carrying the switch to the agent's terminal as the flap does.
const {
  agents,
  clearToasts,
  dockTerminals,
  expectNoCover,
  expectPanePainted,
  expectVisible,
  goto,
  showAgent,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "phone-theater-pill.png",
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
    // The pill is the mode's only way back, so it is the picture; a flight that
    // never started leaves the ordinary pane screen, which crops the same.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisible(page, '[data-testid="theater-pill"]', "the floating pill")
    await expectVisible(
      page,
      '[data-testid="theater-pill"] [aria-label^="Show terminals (1)"]',
      "the switch riding in the pill",
    )
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
