// The macro popover open over an agent pane, listing what that pane can send.
const { agents, boxOf, clearToasts, clickLabel, goto, takeOver } = require("../lib.js")

module.exports = {
  file: "macro-popover.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["review-billing"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "Run a macro")
    // Framed on the pane rather than on the whole window: the crop starts at the
    // terminal pane's own left edge and ends below the popover.
    const pane = await boxOf(page, ['[data-testid="terminal-pane"]'])
    const popover = await boxOf(page, ['[data-testid="macro-popover"],[role="dialog"]'])
    const x = Math.round(pane.x)
    return {
      x,
      y: 0,
      width: Math.min(1440 - x, Math.round(pane.width)),
      height: Math.min(900, Math.round(popover.y + popover.height + 150)),
    }
  },
}
