// Creating an agent from a pull request, one of the New dialog's other
// ways in.
const {
  agents,
  boxOf,
  clearToasts,
  expectDialogOpen,
  expectFieldValue,
  goto,
  openLauncherRow,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "new-agent-from-pr-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    // Opened over an agent so the crop keeps a terminal behind it.
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openLauncherRow(page, "new-agent-from-pr")
    await sleep(1400)
    await page.evaluate(() => document.querySelector('[role="dialog"] input')?.focus())
    await page.keyboard.type("acme/demo-api#123", { delay: 25 })
    await sleep(700)
    await clearToasts(page)
    // The dialog the menu item opens, with the reference typed into its first
    // field, which is the whole of what the caption promises.
    await expectDialogOpen(page, "New agent from PR")
    await expectFieldValue(page, '[role="dialog"] input', "acme/demo-api#123")
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
