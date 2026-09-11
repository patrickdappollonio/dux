// The worktree manager, reached as the way to adopt a worktree that no agent
// holds.
const {
  agents,
  boxOf,
  clearToasts,
  clickText,
  expectDialogOpen,
  expectVisibleText,
  goto,
  openMoreWays,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "worktree-manager-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openMoreWays(page)
    await clickText(page, "New agent from existing worktree", "[role=menuitem]")
    await sleep(1200)
    await clickText(page, "demo-api", '[role="dialog"] button')
    await sleep(1600)
    await clearToasts(page)
    // The manager itself rather than the project picker it is reached through,
    // and one of the worktrees an agent is holding, which is what the caption
    // says the list names.
    await expectDialogOpen(page, "Worktrees in demo-api")
    await expectVisibleText(page, "fix-login-redirect", { what: "a worktree in the list" })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
