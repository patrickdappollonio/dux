// The virtual input: the terminal-key rows and the compose bar a finger types
// into, with a draft in it.
const { agents, clearToasts, goto, sleep, takeOver } = require("../lib.js")

module.exports = {
  file: "phone-compose-bar.png",
  viewport: "phone",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await sleep(1500)
    await page.evaluate(() => document.querySelector("textarea")?.focus())
    await page.keyboard.type("Add a test for the redirect loop", { delay: 20 })
    await sleep(800)
    await clearToasts(page)
    // The virtual input is the key rows plus the compose bar: from the top of
    // the first key row to the bottom of the viewport.
    const top = await page.evaluate(() => {
      const esc = [...document.querySelectorAll("button")].find(
        (b) => (b.textContent || "").trim() === "Esc",
      )
      return esc ? Math.round(esc.getBoundingClientRect().top - 10) : null
    })
    if (top == null) throw new Error("no compose bar on this pane")
    return { x: 0, y: top, width: 390, height: 844 - top }
  },
}
