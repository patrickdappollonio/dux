// The sign-in page reached over plain HTTP from another device, with the red
// warning that anyone on the network in between can read the password.
const { clearToasts, expectVisible, expectVisibleText, goto } = require("../lib.js")
const { gateClip, requirePassword, resetAuth } = require("../auth.js")

module.exports = {
  file: "login-plain-http.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword()
    await goto(page, "")
    await clearToasts(page)
    await expectVisibleText(page, "Sign in to dux")
    await expectVisible(page, '[data-testid="login-form"]', "the password form")
    await expectVisible(page, '[data-testid="login-insecure-warning"]', "the plain-HTTP warning")
    return gateClip(page)
  },
}
