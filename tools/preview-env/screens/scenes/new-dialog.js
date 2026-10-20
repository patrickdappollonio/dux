// The launcher's New… button pressed: every way to create something, in its
// three groups, each row with the sentence that says what it makes.
const {
  agents,
  boxOf,
  clearToasts,
  expectDialogOpen,
  expectVisibleText,
  goto,
  openLauncher,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "new-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    // Opened over an agent so the crop keeps a terminal behind it.
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openLauncher(page)
    await sleep(600)
    // The first row takes focus on open; the picture is of the list at rest.
    await page.evaluate(() => document.activeElement?.blur())
    await sleep(300)
    await clearToasts(page)
    // The dialog with all three groups, the last group's last row included,
    // which is what the caption says the picture shows.
    await expectDialogOpen(page, "Start something new")
    for (const title of ["New agent from PR", "New terminal for an agent", "Initialize a repository"]) {
      await expectVisibleText(page, title, { selector: '[role="dialog"] button span' })
    }
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
