// The sign-in page with no warning on it, the way a browser on an encrypted
// path sees it.
//
// The preview's browser reaches dux over plain HTTP through Docker's published
// port, and nothing in the preview can put it on a tailnet or behind HTTPS. So
// this scene sets `cookie_secure = "always"`, which is the docs' own answer for
// an HTTPS proxy dux cannot see into: it tells dux browsers reach it encrypted,
// and dux leaves the warning off exactly as it does on a trusted path. The page
// is otherwise the one every such visitor gets.
const { clearToasts, expectVisible, expectVisibleText, goto, refusal } = require("../lib.js")
const { gateClip, requirePassword, resetAuth } = require("../auth.js")

module.exports = {
  file: "login-trusted.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword({ cookieSecure: "always" })
    await goto(page, "")
    await clearToasts(page)
    await expectVisibleText(page, "Sign in to dux")
    await expectVisible(page, '[data-testid="login-form"]', "the password form")
    // The whole point of this picture: the warning is absent.
    if (await page.$('[data-testid="login-insecure-warning"]')) {
      throw refusal("login-trusted: the plain-HTTP warning is on the page")
    }
    return gateClip(page)
  },
}
