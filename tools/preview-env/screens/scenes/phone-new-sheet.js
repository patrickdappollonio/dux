// The New… button on a phone: the same list as the desktop dialog, as a sheet
// over the hub.
const {
  clearToasts,
  expectDialogOpen,
  expectVisibleText,
  goto,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "phone-new-sheet.png",
  viewport: "phone",
  async shoot(page) {
    await goto(page, "")
    await sleep(1200)
    await clearToasts(page)
    // A DOM click rather than a pointer one: under touch emulation the
    // emulated tap never reports done once the sheet covers the button.
    await page.evaluate(() =>
      [...document.querySelectorAll('button[aria-label="New…"]')]
        .find((b) => b.getBoundingClientRect().width > 40)
        ?.click(),
    )
    await sleep(900)
    await page.evaluate(() => document.activeElement?.blur())
    await sleep(300)
    await clearToasts(page)
    // The sheet with its title and the Agents group the caption names.
    await expectDialogOpen(page, "Start something new")
    await expectVisibleText(page, "New standalone agent", { selector: '[role="dialog"] button span' })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
