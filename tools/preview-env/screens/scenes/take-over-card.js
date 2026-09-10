// The full-pane card a watcher gets over a terminal somebody else is driving.
// This scene needs a SECOND device on the same pty, so it opens the driver
// itself; the page it is handed is the watcher, which is why it must not have
// been anywhere near the agent yet.
const { agents, clearToasts, goto, open, sleep, takeOver } = require("../lib.js")

module.exports = {
  file: "take-over-card.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    const sid = by["add-rate-limits"].id

    const driver = await open({})
    try {
      await goto(driver.page, `#/agent/${sid}`)
      await takeOver(driver.page)
      await sleep(2000)

      await goto(page, `#/agent/${sid}`)
      await sleep(3500)
      await clearToasts(page)
      const clip = await page.evaluate(() => {
        const btn = [...document.querySelectorAll("button")].find((b) =>
          /take over/i.test(b.textContent || ""),
        )
        if (!btn) return null
        // Walk out to the card itself: the button's own wrapper is nearly as
        // wide, so the height is what tells them apart.
        let card = btn
        while (card && card.getBoundingClientRect().height < 150) card = card.parentElement
        const r = card.getBoundingClientRect()
        return {
          x: Math.round(r.x - 8),
          y: Math.round(r.y - 12),
          width: Math.round(r.width + 16),
          height: Math.round(r.height + 24),
        }
      })
      if (!clip) throw new Error("no take-over card on the watcher")
      return clip
    } finally {
      await driver.browser.close()
    }
  },
}
