// One tab's own menu, open over the strip.
const {
  agents,
  boxOf,
  clearToasts,
  clickLabel,
  expectMenuOpen,
  expectNoCover,
  expectPanePainted,
  goto,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "agent-tab-actions-menu.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["refactor-cache"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "Tab actions", 2)
    // The menu the caption names, over a terminal that is actually on.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectMenuOpen(page, "Close tab")
    // The strip container runs the full pane width, so the right edge comes from
    // the controls rather than from it: whichever of the chevron and the open
    // menu ends furthest right.
    const strip = await page.evaluate(() => {
      const el = document.querySelector('[aria-label="New tab"]').closest("div").parentElement
      const b = el.getBoundingClientRect()
      return { x: b.x - 6, y: b.y - 8, bottom: b.bottom + 8 }
    })
    const chevron = await boxOf(page, ['[aria-label="Choose provider for new tab"]'])
    const menu = await boxOf(page, ['[role="menu"]'])
    const x = Math.max(0, Math.round(Math.min(strip.x, menu.x)))
    const y = Math.max(0, Math.round(Math.min(strip.y, menu.y)))
    const right = Math.max(chevron.x + chevron.width + 10, menu.x + menu.width + 10)
    const bottom = Math.max(strip.bottom, menu.y + menu.height + 8)
    return {
      x,
      y,
      width: Math.min(1440 - x, Math.round(right - x)),
      height: Math.min(900 - y, Math.round(bottom - y)),
    }
  },
}
