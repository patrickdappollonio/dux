// The Pull requests dialog: the agent's pull requests in its order, each with a
// grip to drag and a menu, the Sort control and the way to attach another.
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
  file: "pull-requests-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "All pull requests")
    await clickText(page, "^Pull requests…$", "[role=menuitem]")
    await sleep(900)
    await clearToasts(page)
    await expectDialogOpen(page, "Pull requests")
    await expectVisibleText(page, "Allow the preview host in the ingress", {
      what: "the attached pull request's row",
    })
    await expectVisibleText(page, "login-session-cookie", {
      what: "the merged pull request's branch",
    })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
