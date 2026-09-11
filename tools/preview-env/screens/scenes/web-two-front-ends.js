// The browser half of the introduction's pair: the same workspace the terminal
// UI shot shows, three agents working and three idle.
const {
  agents,
  clearToasts,
  displayOrder,
  expectNoCover,
  expectPanePainted,
  expectRows,
  expectStateWord,
  freshen,
  goto,
  sleep,
  takeOver,
} = require("../lib.js")

module.exports = {
  file: "web-two-front-ends.png",
  viewport: "desktop",
  staging: "twin",
  async shoot(page) {
    const by = await agents()
    await freshen(by["add-rate-limits"].id, 0)
    await goto(page, `#/agent/${by["add-rate-limits"].id}`)
    await takeOver(page)
    // The Terminals section is collapsed here so all six agents fit the pane,
    // which is what the caption counts. Pressed until the divider says it is
    // closed rather than once: the divider is rendered from the live view model,
    // so a single click aimed at it before the terminals have arrived presses
    // nothing at all and the section is still open at the shutter, which is how
    // this picture came back carrying two terminal rows the caption never
    // mentions.
    let closed = false
    for (let i = 0; i < 20 && !closed; i++) {
      const state = await page.evaluate(() => {
        const btn = [...document.querySelectorAll("button")].find((b) =>
          /^\s*Terminals/.test(b.textContent || ""),
        )
        if (!btn) return "absent"
        if (btn.getAttribute("aria-expanded") === "false") return "closed"
        btn.click()
        return "clicked"
      })
      closed = state === "closed"
      if (!closed) await sleep(500)
    }
    if (!closed) throw new Error("the Terminals section never closed")
    await sleep(2500)
    await clearToasts(page)
    // The six rows the caption counts beside its terminal UI twin, and the one
    // whose pane is on screen. Half this staging's agents are idle, so the
    // default active-first sort shows them after the three still working; the
    // pinned order is asked for through the sort rather than read as the answer.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectRows(page, displayOrder("twin"))
    await expectStateWord(page, "add-rate-limits", "Working")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
