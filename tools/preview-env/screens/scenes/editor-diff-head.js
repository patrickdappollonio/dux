// The editor's diff mode: the working copy beside what HEAD has.
const {
  agents,
  boxOf,
  clearToasts,
  expectVisible,
  expectVisibleText,
  goto,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "editor-diff-head.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(
      page,
      `#/editor/agent/${by["add-rate-limits"].id}/diff/${encodeURIComponent("src/main.py")}`,
    )
    await sleep(4000)
    await clearToasts(page)
    // The banner that says which version is on the left, the file it is about,
    // and a diff editor that actually mounted: monaco loading late leaves an
    // empty frame that crops perfectly well.
    await expectVisible(page, '[data-testid="diff-head-banner"]', "the diff-against-HEAD banner")
    await expectVisibleText(page, "main.py", { what: "the file the diff is of" })
    await expectVisible(page, ".monaco-editor", "the diff editor")
    // The viewer is monaco's diff editor; the crop is its own rect, cut at the
    // height that leaves the hunk filling it.
    const d = await boxOf(page, [".monaco-diff-editor", ".monaco-editor"])
    return { x: d.x, y: d.y, width: d.width, height: Math.min(376, d.height) }
  },
}
