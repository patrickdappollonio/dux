// The full-pane card a watcher gets over a terminal somebody else is driving.
// This scene needs a SECOND device on the same pty, so it opens the driver
// itself; the page it is handed is the watcher, which is why it must not have
// been anywhere near the agent yet.
const {
  agents,
  clearToasts,
  expectNoCover,
  expectVisibleText,
  goto,
  open,
  sleep,
  takeOver,
} = require("../lib.js")

// The card's own words, or empty when there is no card over the terminal.
const cardText = (page) =>
  page.evaluate(() => {
    const btn = [...document.querySelectorAll("button")].find((b) =>
      /take over/i.test(b.textContent || ""),
    )
    if (!btn) return ""
    let card = btn
    while (card && card.getBoundingClientRect().height < 150) card = card.parentElement
    return (card.textContent || "").slice(0, 160)
  })

module.exports = {
  file: "take-over-card.png",
  viewport: "desktop",
  async shoot(page, ctx) {
    const by = await agents()
    const sid = by["add-rate-limits"].id

    const driver = await open({})
    // Closed after the capture, not before it: the owner disconnecting re-titles
    // the card to the nobody-is-driving wording, which is a different picture.
    ctx.after(() => driver.browser.close())
    {
      await goto(driver.page, `#/agent/${sid}`)
      // Whether the driver arrives at an unowned pty (no card, claimed by the
      // plain attach) or at one somebody else holds (a card, and the button is
      // what claims it), it has to be the owner before the watcher arrives, or
      // the watcher gets the nobody-is-driving card instead of this one.
      await takeOver(driver.page)
      await sleep(2000)
      const driverCard = await cardText(driver.page)
      if (driverCard) throw new Error(`the driver never took the pty: ${driverCard}`)

      await goto(page, `#/agent/${sid}`)
      await sleep(3500)
      await clearToasts(page)
      // The card has a word for a pty nobody drives too, and that is a different
      // picture from the one the docs caption. Refuse it rather than write it.
      // The one scene where the card is the subject, so the cover guard is
      // asked for it rather than against it; the spinner and the reconnect box
      // are refused here like everywhere else.
      await expectNoCover(page, { card: true })
      await expectVisibleText(page, "Take over", { what: "the card's button" })
      const watcherCard = await cardText(page)
      if (!/Active on/i.test(watcherCard)) {
        throw new Error(`the watcher's card does not name a driving device: ${watcherCard}`)
      }
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
    }
  },
}
