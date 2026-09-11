// A file held over an agent pane, with the drop overlay saying where it will
// land.
const {
  agents,
  boxOf,
  clearToasts,
  expectNoCover,
  expectPanePainted,
  expectVisible,
  freshen,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "file-drop-overlay.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    // The overlay dims what is behind it, so the pane is restarted first:
    // minutes of accumulated output read as a grey wall rather than a session.
    await freshen(by["add-rate-limits"].id, 2)
    await goto(page, `#/agent/${by["add-rate-limits"].id}`)
    await takeOver(page)
    await clearToasts(page)
    // Read BEFORE the drag, deliberately: the overlay paints over the terminal
    // at nine tenths opacity, so afterwards the pane's own pixels are the
    // overlay's and say nothing about the session behind it.
    await expectNoCover(page)
    // Asked once, and deliberately not again at the shutter: by then the overlay
    // is painted over the terminal at nine tenths opacity, so a second reading
    // measures the overlay's own pixels rather than the session behind it. The
    // shared floor applies, because the pane is not as sparse as the two-second
    // restart suggests: it measures a couple of percent here.
    await expectPanePainted(page, { once: true })
    // A real drag through CDP: the pane's gate reads dataTransfer.types, which a
    // hand-built DragEvent does not populate the same way.
    const cdp = await page.createCDPSession()
    const at = await boxOf(page, ['[data-testid="terminal-container"]'])
    const data = {
      items: [{ mimeType: "image/png", data: "iVBORw0KGgo=" }],
      files: ["/tmp/shot.png"],
      dragOperationsMask: 1,
    }
    const point = { x: Math.round(at.x + at.width / 2), y: Math.round(at.y + at.height / 2) }
    await cdp.send("Input.dispatchDragEvent", { type: "dragEnter", ...point, data })
    await cdp.send("Input.dispatchDragEvent", { type: "dragOver", ...point, data })
    await sleep(900)
    // The overlay is the whole picture, and a drag the pane's gate refused
    // leaves an ordinary terminal that crops just as well.
    await expectVisible(page, '[data-testid="file-drop-overlay"]', "the file-drop overlay")
    // Cropped to the PANE, not to the overlay: the overlay is inset inside the
    // pane and its dashed border runs along that inset, so cropping to the
    // overlay puts the border on the crop edge and slices the terminal's first
    // column. The pane's own bounds leave clear margin on all four sides.
    const pane = await boxOf(page, ['[data-testid="terminal-pane"]'])
    const overlay = await boxOf(page, ['[data-testid="file-drop-overlay"]'])
    const x = Math.max(0, Math.round(Math.min(pane.x, overlay.x - 8)))
    const y = Math.max(0, Math.round(Math.min(pane.y, overlay.y - 8)))
    const right = Math.max(pane.x + pane.width, overlay.x + overlay.width + 8)
    const width = Math.min(1440 - x, Math.round(right - x))
    // Square, deliberately: the overlay is centred in the pane and a square crop
    // leaves it the same air above and below as it has either side.
    return { x, y, width, height: Math.min(900 - y, width) }
  },
}
