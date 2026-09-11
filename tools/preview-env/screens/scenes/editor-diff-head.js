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
    // The file, and a DIFF editor rather than a plain one: monaco loading late
    // leaves an empty frame that crops perfectly well, and an editor with one
    // side is a picture of a file rather than of a comparison. The mode's own
    // banner is deliberately not asked for; it speaks only when it has
    // something to say, and it is above this crop either way.
    await expectVisibleText(page, "main.py", { what: "the file the diff is of" })
    await expectVisible(page, ".monaco-diff-editor", "the diff editor")
    // The viewer is monaco's diff editor; the crop is its own rect, cut at the
    // height that leaves the hunk filling it.
    const d = await boxOf(page, [".monaco-diff-editor", ".monaco-editor"])
    return { x: d.x, y: d.y, width: d.width, height: Math.min(376, d.height) }
  },
}
