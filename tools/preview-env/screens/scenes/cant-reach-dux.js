// The page that says the browser could not ask dux whether it needs a password.
//
// The page shows when that first question goes unanswered until the browser's
// request deadline (a request that fails at once opens the app instead, whose
// offline overlay already says dux is down). The page itself has to come from
// dux, so dux cannot simply be stopped; the scene holds the page's status read
// and never answers it, which is what a dux that has stopped answering looks
// like from the browser.
const { BASE, clearToasts, expectVisibleText, sleep } = require("../lib.js")
const { gateClip } = require("../auth.js")

module.exports = {
  file: "cant-reach-dux.png",
  viewport: "desktop",
  async shoot(page) {
    await page.setRequestInterception(true)
    page.on("request", (request) => {
      // Left pending: neither continued nor answered.
      if (new URL(request.url()).pathname === "/api/v1/auth/status") return
      request.continue()
    })
    // Not lib's goto: it waits for the network to go idle, and the held request
    // keeps it busy until the page gives up on it.
    await page.goto(`${BASE}/`, { waitUntil: "domcontentloaded", timeout: 45000 })
    // The browser's default deadline is ten seconds.
    await sleep(12000)
    await clearToasts(page)
    await expectVisibleText(page, "Can't reach dux")
    await expectVisibleText(page, "Retry", { selector: "button" })
    return gateClip(page)
  },
}
