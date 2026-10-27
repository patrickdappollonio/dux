// The launcher corner with its ▾ open: the New agent… half, and the menu of
// every other way to start, each entry with the sentence that says what it does.
const {
  agents,
  boxOf,
  clearToasts,
  expectVisible,
  expectVisibleText,
  goto,
  openLauncherMenu,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "launcher-menu.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    // Opened over an agent so the crop keeps the sidebar it sits in.
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await openLauncherMenu(page)
    await sleep(600)
    await clearToasts(page)
    // The corner's main half and the menu's three groups, the last entry
    // included, which is what the caption says the picture shows.
    await expectVisible(page, '[role="menu"]', "the launcher menu")
    await expectVisibleText(page, "New agent…", { selector: '[data-sidebar="footer"] button' })
    for (const title of ["New agent from PR…", "New standalone terminal", "Initialize a repository…"]) {
      await expectVisibleText(page, title, { selector: '[role="menu"] span' })
    }
    return await boxOf(page, ['[role="menu"]', '[data-sidebar="footer"]'], 12)
  },
}
