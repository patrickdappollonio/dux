// The browser half of the introduction's pair: the same workspace the terminal
// UI shot shows, three agents working and three idle.
const {
  SIDEBAR_ORDER,
  agents,
  clearToasts,
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
    // which is what the caption counts.
    await page.evaluate(() => {
      const btn = [...document.querySelectorAll("button")].find((b) =>
        /^\s*Terminals/.test(b.textContent || ""),
      )
      if (btn && btn.getAttribute("aria-expanded") === "true") btn.click()
    })
    await sleep(2500)
    await clearToasts(page)
    // The six rows the caption counts beside its terminal UI twin, and the one
    // whose pane is on screen.
    await expectNoCover(page)
    await expectPanePainted(page)
    await expectRows(page, SIDEBAR_ORDER)
    await expectStateWord(page, "add-rate-limits", "Working")
    return { x: 0, y: 0, width: 1440, height: 900 }
  },
}
