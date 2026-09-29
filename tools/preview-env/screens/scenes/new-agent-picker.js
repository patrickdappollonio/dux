// The corner's filled verb, stopped at the project picker it opens first: the
// projects with their agent counts, and the two ways out at its foot.
const {
  boxOf,
  clearToasts,
  expectDialogOpen,
  expectVisibleText,
  goto,
  openNewAgent,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "new-agent-picker.png",
  viewport: "desktop",
  async shoot(page) {
    await goto(page, "")
    await openNewAgent(page)
    await sleep(1200)
    // The picker's search field takes focus on open; the picture is of the
    // list, not of a caret, so it is let go.
    await page.evaluate(() => document.activeElement?.blur())
    await sleep(300)
    await clearToasts(page)
    // The picker itself rather than the naming step a stray click would reach:
    // the demo project's row with its count, and both footer doors, which is
    // what the caption says the picture shows.
    await expectDialogOpen(page, "New agent")
    await expectVisibleText(page, "Choose a project", { selector: '[role="dialog"] *' })
    await expectVisibleText(page, "demo-api", {
      selector: '[role="dialog"] [data-testid="project-row"] *',
      what: "the demo project's row",
    })
    await expectVisibleText(page, "Add a new project…", { selector: '[role="dialog"] button' })
    await expectVisibleText(page, "Add standalone agent…", { selector: '[role="dialog"] button' })
    return await boxOf(page, ['[role="dialog"]'], 12)
  },
}
