// The banner's list: every pull request the agent shows, each a link, and the
// way into the Pull requests dialog.
const {
  agents,
  boxOf,
  clearToasts,
  clickLabel,
  expectMenuOpen,
  expectNoCover,
  expectPanePainted,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "pr-banner-list.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await expectNoCover(page)
    await expectPanePainted(page)
    await clickLabel(page, "All pull requests")
    await sleep(600)
    // All three are in it, the attached one under its repository's name.
    await expectMenuOpen(page, "acme/infra#77")
    await expectMenuOpen(page, "#118")
    await expectMenuOpen(page, "Pull requests…")
    return await boxOf(page, ['[data-testid="pr-banner"]', '[role="menu"]'], 12)
  },
}
