// Creating an agent from a pull request, one of the launcher ellipsis's other
// ways in.
const {
  agents,
  boxOf,
  clearToasts,
  clickText,
  goto,
  openMoreWays,
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
    await openMoreWays(page)
    await clickText(page, "New agent from PR", "[role=menuitem]")
    await sleep(1400)
    await page.evaluate(() => document.querySelector('[role="dialog"] input')?.focus())
    await page.keyboard.type("acme/demo-api#123", { delay: 25 })
    await sleep(700)
    await clearToasts(page)
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
