// The launcher corner's "New terminal for an agent…": which agent's copy of the
// code a new terminal opens in, every agent under its project and the
// standalone one last.
const {
  agents,
  boxOf,
  clearToasts,
  expectDialogOpen,
  expectVisible,
  expectVisibleText,
  goto,
  openLauncherEntry,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "new-terminal-for-agent.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openLauncherEntry(page, "new-terminal-for-agent")
    await sleep(600)
    await clearToasts(page)
    // The step itself and both kinds of group the caption names.
    await expectDialogOpen(page, "New terminal for an agent")
    await expectVisible(page, '[role="dialog"] input[aria-label="Search agents"]', "the search field")
    await expectVisibleText(page, "Standalone", { selector: '[role="dialog"] span' })
    await expectVisibleText(page, "design-notes", { selector: '[role="dialog"] button span' })
    await expectVisibleText(page, "fix-login-redirect", { selector: '[role="dialog"] button span' })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
