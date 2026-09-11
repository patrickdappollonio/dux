// The corner's filled verb, through the project picker, to the naming step.
const {
  boxOf,
  clearToasts,
  clickText,
  expectDialogOpen,
  expectFieldValue,
  goto,
  openNewAgent,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "new-agent-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    await goto(page, "")
    await openNewAgent(page)
    // Scoped to the picker: every sidebar row carries the project name too.
    await clickText(page, "demo-api", '[role="dialog"] button')
    await sleep(1400)
    await page.evaluate(() => document.querySelector('[role="dialog"] input')?.focus())
    await page.keyboard.type("add-rate-limits-v2", { delay: 25 })
    await sleep(700)
    await clearToasts(page)
    // The naming step rather than the picker it came through, with the typed
    // branch name in the field: an unfocused field swallows every keystroke and
    // leaves a dialog that crops exactly the same.
    await expectDialogOpen(page, "New agent")
    await expectFieldValue(page, '[role="dialog"] input', "add-rate-limits-v2")
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
