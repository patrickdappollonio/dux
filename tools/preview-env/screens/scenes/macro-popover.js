// The macro popover open over an agent pane, listing what that pane can send.
const {
  agents,
  armAttention,
  boxOf,
  clearToasts,
  clickLabel,
  expectNoCover,
  expectPanePainted,
  expectVisibleText,
  goto,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "macro-popover.png",
  viewport: "desktop",
  async shoot(page, ctx) {
    const by = await agents()
    // Opening this pane clears the agent's needs-you flag, which two other
    // scenes are about. Put it back once the capture is done, so no scene's
    // picture depends on whether this one ran before it.
    ctx.after(() => armAttention())
    await goto(page, `#/agent/${by["review-billing"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "Run a macro")
    // The picker, two of the macros it lists, and the way out to the editor the
    // caption names, over a terminal that is on.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectVisibleText(page, "Write tests", { what: "a macro in the picker" })
    await expectVisibleText(page, "Edit macros", { what: "the picker's editor link" })
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
