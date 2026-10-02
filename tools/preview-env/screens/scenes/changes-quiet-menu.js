// A standalone agent whose folder has no repository: the Changes pane keeps its
// title and its ⋯, and the menu greys out the folder's own git items with the
// reason at its head while Hide Changes pane stays live.
const {
  agents,
  boxOf,
  clearToasts,
  clickLabel,
  expectMenuOpen,
  expectNoCover,
  expectVisibleText,
  goto,
  sleep,
  steadyBox,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "changes-quiet-menu.png",
  viewport: "desktop",
  staging: "standalone",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["design-notes"].id}`)
    await takeOver(page)
    await clearToasts(page)
    await clickLabel(page, "Changes actions")
    await sleep(900)
    // The quiet sentence under the header, the menu open over it, the reason
    // at the menu's head and the one item that must stay live.
    await expectNoCover(page)
    await expectVisibleText(page, "no git repository, so there are no changes", {
      what: "the quiet sentence under the header",
    })
    await expectMenuOpen(page, "Hide Changes pane")
    await expectVisibleText(page, "This folder has no git repository.", {
      selector: '[role="menu"] *',
      what: "the reason at the head of the menu",
    })
    const pane = await boxOf(page, ['[data-testid="changes-pane"]'])
    const menu = await steadyBox(() => boxOf(page, ['[role="menu"]']), {
      what: "the Changes pane's menu",
    })
    // The pane from its top down to a little past the quiet sentence and the
    // folder under it, which sit lower than the menu, so the frame holds both.
    // The header block rather than the whole empty state, which fills the pane.
    const sentence = await boxOf(page, [
      '[data-testid="changes-pane"] [data-slot="empty-header"]',
    ])
    const bottom = Math.max(menu.y + menu.height, sentence.y + sentence.height) + 16
    return {
      x: Math.round(pane.x),
      y: Math.round(pane.y),
      width: Math.round(pane.width),
      height: Math.min(900 - Math.round(pane.y), Math.round(bottom - pane.y)),
    }
  },
}
