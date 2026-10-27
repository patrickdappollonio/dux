// The launcher corner's ▾ on a phone: every other way to start, as a sheet over
// the hub, each entry with its sentence.
const {
  clearToasts,
  expectVisible,
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
      document.querySelector('button[aria-label="More ways to start"]')?.click(),
    )
    await sleep(900)
    await page.evaluate(() => document.activeElement?.blur())
    await sleep(300)
    await clearToasts(page)
    // The sheet with the Agents group the caption names and its sentences.
    await expectVisible(page, '[role="menu"]', "the launcher menu's sheet")
    await expectVisibleText(page, "New standalone agent…", { selector: '[role="menu"] span' })
    await expectVisibleText(page, "Run an agent in any folder, no project or git needed.", {
      selector: '[role="menu"] span',
    })
    return { x: 0, y: 0, width: 390, height: 844 }
  },
}
