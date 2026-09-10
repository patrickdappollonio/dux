// The tab strip above an agent's terminal, one pill per provider tab.
const { agents, boxOf, clearToasts, goto, takeOver } = require("../lib.js")

module.exports = {
  file: "agent-tabs-strip.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["refactor-cache"].id}`)
    await takeOver(page)
    await clearToasts(page)
    // The strip's own container gives the vertical band; its right edge is all
    // trailing space, so the crop ends just past the provider chevron.
    const strip = await page.evaluate(() => {
      const el = document.querySelector('[aria-label="New tab"]').closest("div").parentElement
      const b = el.getBoundingClientRect()
      return { x: b.x, y: b.y, height: b.height }
    })
    const chevron = await boxOf(page, ['[aria-label="Choose provider for new tab"]'])
    const x = Math.max(0, Math.round(strip.x - 6))
    const y = Math.max(0, Math.round(strip.y - 8))
    return {
      x,
      y,
      width: Math.round(chevron.x + chevron.width + 10 - x),
      height: Math.round(strip.height + 16),
    }
  },
}
