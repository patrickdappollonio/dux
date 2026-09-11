// A standalone agent selected: the row wears the star over its folder, the
// crumb names the folder rather than a branch, and the changes panel says why it
// has nothing to show.
const {
  agents,
  clearToasts,
  expectNoCover,
  expectPanePainted,
  expectRows,
  expectVisibleText,
  freshen,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

// The transcript provider ticks a counter forever, so the elapsed figure in its
// last line is whatever the session has been up for. The session is restarted
// and given exactly this long, which pins the number to the second it is
// counting in; it is the one value in the committed set that a reshoot can move
// by one.
// The rest of this scene (the navigation, the take-over, the typing) adds about
// fifteen seconds of its own, which is why this number is small.
const TRANSCRIPT_SECONDS = 4

module.exports = {
  file: "sidebar-standalone.png",
  viewport: "desktop",
  staging: "standalone",
  async shoot(page) {
    const by = await agents()
    const standalone = by["design-notes"]
    await freshen(standalone.id, TRANSCRIPT_SECONDS)
    await goto(page, `#/agent/${standalone.id}`)
    await takeOver(page)
    // The sidebar is filtered, which is what leaves the standalone star and its
    // folder line as the only row in the pane.
    await page.click('input[placeholder*="Search agents"]')
    await page.type('input[placeholder*="Search agents"]', "design", { delay: 40 })
    await sleep(1200)
    await clearToasts(page)
    // The filter leaving one row, the folder the row names instead of a
    // project, and the sentence the changes panel answers a plain folder with.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectRows(page, ["design-notes"])
    await expectVisibleText(page, "design-notes", { what: "the folder the row names" })
    await expectVisibleText(page, "git repository", {
      what: "the changes panel's word about a plain folder",
    })
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
