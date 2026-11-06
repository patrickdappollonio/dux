// The Pull requests dialog's Branches view: every branch the worktree has been
// on, the current one first, each with the pull requests found on it, and the
// attached one that belongs to none of them under its own heading.
const {
  agents,
  boxOf,
  clearToasts,
  clickLabel,
  clickText,
  expectDialogOpen,
  expectVisibleText,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "pull-requests-branches.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "All pull requests")
    await clickText(page, "^Pull requests…$", "[role=menuitem]")
    await sleep(900)
    await clickText(page, "^Branches", '[role="tab"]')
    await sleep(900)
    await clearToasts(page)
    await expectDialogOpen(page, "Pull requests")
    await expectVisibleText(page, "login-session-cookie", { what: "the earlier branch" })
    await expectVisibleText(page, "Current", { what: "the current branch's label" })
    await expectVisibleText(page, "was attached by you", {
      what: "the attached pull request's heading",
    })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
