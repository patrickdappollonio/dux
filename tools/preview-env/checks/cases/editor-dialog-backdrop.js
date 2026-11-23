// A dialog opened from inside the editor sits on a backdrop of its own, so a
// press beside it lands on that backdrop and closes it rather than reaching
// the editor underneath.

const AGENT = "add-rate-limits"

module.exports = {
  protects:
    "On a computer a dialog opened inside the editor (New file) has its own backdrop and closes on a press outside it, leaving the editor open.",
  async run(ctx, H) {
    const dev = await H.device(ctx, "computer")
    await H.openEditor(dev, await H.agentId(AGENT))
    await H.press(dev, { name: "^New file$", selector: "button" })
    const d = await H.waitForDialog(dev, "^New file")

    // Beside the card, over the editor's file tree.
    const point = { x: Math.max(20, d.box.x / 2), y: d.box.y + d.box.height / 2 }
    const under = await dev.page.evaluate(({ x, y }) => {
      const el = document.elementFromPoint(x, y)
      if (!el) return { backdrop: false, what: "nothing" }
      const inEditor = el.closest("#editor-explorer, button, [role=tree], [role=treeitem]") !== null
      const r = el.getBoundingClientRect()
      return {
        backdrop: !inEditor && r.width >= innerWidth - 1 && r.height >= innerHeight - 1,
        what: `${el.tagName.toLowerCase()} ${Math.round(r.width)}x${Math.round(r.height)} "${(el.textContent || "").trim().slice(0, 40)}"`,
      }
    }, point)
    H.expect(
      under.backdrop,
      `beside the "${d.name}" dialog (at ${Math.round(point.x)},${Math.round(point.y)}) there should be a backdrop covering the editor; the press would land on ${under.what}`,
    )
    await dev.page.mouse.click(point.x, point.y)
    await H.waitForDialogGone(dev, `^${d.name}$`, `"${d.name}" on a press outside it`)
    const editorOpen = await dev.page.evaluate(H.EDITOR_READY)
    H.expect(editorOpen, "the press outside the dialog should leave the editor open; the editor is gone")
  },
}
