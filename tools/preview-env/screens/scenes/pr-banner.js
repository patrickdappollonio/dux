// The pull-request banner: the wide link to the pull request it shows, and the
// counter, arrows and list button at its end that step through the others.
const {
  agents,
  clearToasts,
  expectBanner,
  expectNoCover,
  expectPanePainted,
  expectVisibleText,
  goto,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "pr-banner.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    // The banner across the pane, not the sidebar row's chip, which carries the
    // same words and the same href at a fraction of the width.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectBanner(page, "Fix the login redirect loop", {
      what: "the pull-request banner",
    })
    await expectVisibleText(page, "1 of 3", {
      selector: '[data-testid="pr-banner"] span',
      what: "the banner's counter",
    })
    // The whole strip, its trailing controls included, not just the link.
    const r = await page.evaluate(() => {
      const strip = document.querySelector('[data-testid="pr-banner"]')
      const b = strip && strip.getBoundingClientRect()
      return b ? { x: b.x, y: b.y, width: b.width, height: b.height } : null
    })
    if (!r) throw new Error("no pull-request banner on this pane")
    return {
      x: Math.max(0, Math.round(r.x)),
      y: Math.max(0, Math.round(r.y - 6)),
      width: Math.round(r.width),
      height: Math.round(r.height + 12),
    }
  },
}
