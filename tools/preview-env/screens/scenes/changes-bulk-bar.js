// The Changes pane with two rows ticked and the selection button's menu open.
const {
  agents,
  boxOf,
  clearToasts,
  clickText,
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
    // The pane may be closed, behind its reopen button.
    await page.evaluate(() =>
      document.querySelector('button[aria-label^="Show Changes pane"]')?.click(),
    )
    await sleep(1200)
    // The row checkbox sits invisible behind the status marker until hover, so
    // it is clicked directly rather than through the pointer.
    await page.evaluate(() => {
      const boxes = [
        ...document.querySelectorAll('#changes-pane [role="checkbox"]'),
      ]
      boxes.slice(0, 2).forEach((c) => c.click())
    })
    await sleep(1200)
    // The button exists only while rows are ticked: a click that missed leaves
    // the pane looking ordinary and perfectly croppable.
    await expectNoCover(page)
    await expectVisibleText(page, "selected", { selector: '[role="toolbar"] button', what: "the selection button" })
    await clickText(page, "^\\d+ selected$", '#changes-pane button')
    await sleep(600)
    await expectVisibleText(page, "Stage 2", { selector: '[role="menu"] *', what: "the menu's Stage verb" })
    await expectVisibleText(page, "Discard 2", { selector: '[role="menu"] *', what: "the menu's Discard verb" })
    await expectVisibleText(page, "Select all", { selector: '[role="menu"] *', what: "the menu's Select all" })
    const pane = await boxOf(page, ['#changes-pane'])
    const menu = await boxOf(page, ['[role="menu"]'])
    const filter = await boxOf(page, ['#changes-pane input[type="search"]'])
    const y = Math.max(0, Math.round(filter.y - 16))
    return {
      x: Math.round(pane.x),
      y,
      width: Math.round(pane.width),
      height: Math.min(900 - y, Math.round(menu.y + menu.height + 16 - y)),
    }
  },
}
