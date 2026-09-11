// The Changes pane with two rows ticked, so the bulk bar of plain verbs is up.
const {
  agents,
  boxOf,
  clearToasts,
  expectNoCover,
  expectVisibleText,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "changes-bulk-bar.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    // The row checkbox is a visually hidden input behind the status marker, so
    // it is clicked directly rather than through the pointer.
    await page.evaluate(() => {
      const boxes = [
        ...document.querySelectorAll('[data-testid="changes-pane"] input[type="checkbox"]'),
      ]
      boxes.slice(0, 2).forEach((c) => c.click())
    })
    await sleep(1200)
    // The bar is the picture, and it exists only while rows are ticked: a click
    // that missed leaves the pane looking ordinary and perfectly croppable.
    await expectNoCover(page)
    await expectVisibleText(page, "Stage 2", { what: "the bulk bar's Stage verb" })
    await expectVisibleText(page, "Discard 2", { what: "the bulk bar's Discard verb" })
    const pane = await boxOf(page, ['[data-testid="changes-pane"]'])
    const filter = await boxOf(page, ['[data-testid="changes-pane"] input[type="search"]'])
    const rows = await boxOf(page, ['[data-testid="changes-pane"] [aria-label^="Actions for "]'])
    const y = Math.max(0, Math.round(filter.y - 16))
    return {
      x: Math.round(pane.x),
      y,
      width: Math.round(pane.width),
      height: Math.min(900 - y, Math.round(rows.y + rows.height + 16 - y)),
    }
  },
}
