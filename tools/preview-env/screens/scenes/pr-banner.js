// The pull-request banner, the wide anchor that opens the PR from the pane.
const {
  agents,
  clearToasts,
  expectBanner,
  expectNoCover,
  expectPanePainted,
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
    // The sidebar row's chip carries the same href at chip width, so the banner
    // is picked out as the widest of the pull-request anchors.
    const r = await page.evaluate(() => {
      const wide = [...document.querySelectorAll('a[href*="/pull/"]')]
        .map((a) => a.getBoundingClientRect())
        .filter((b) => b.width > 300)
        .sort((a, b) => b.width - a.width)[0]
      return wide ? { x: wide.x, y: wide.y, width: wide.width, height: wide.height } : null
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
