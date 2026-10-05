// The page a browser gets if a broken [server.auth] section ever reaches a
// running dux.
//
// dux itself never gets there: a broken section stops the start and a reload
// refuses it, so the page is the fail-closed answer to a state no real run can
// be put in. The scene therefore answers the page's one status read itself,
// with the server's real answer for this browser and the one field the broken
// state changes: `auth_broken`, holding the sentence dux sends a browser outside
// this machine and its tailnet (the exact problem goes only to those).
// Everything on the page is the real component rendering that answer.
const { BASE, clearToasts, expectVisibleText, goto } = require("../lib.js")
const { gateClip } = require("../auth.js")

module.exports = {
  file: "broken-config-page.png",
  viewport: "desktop",
  async shoot(page) {
    const real = await (await fetch(`${BASE}/api/v1/auth/status`)).json()
    await page.setRequestInterception(true)
    page.on("request", (request) => {
      if (new URL(request.url()).pathname !== "/api/v1/auth/status") {
        request.continue()
        return
      }
      request.respond({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ ...real, auth_broken: "The [server.auth] section of dux's config is invalid." }),
      })
    })
    await goto(page, "")
    await clearToasts(page)
    await expectVisibleText(page, "Sign-in is misconfigured")
    await expectVisibleText(page, "Try again", { selector: "button" })
    return gateClip(page)
  },
}
