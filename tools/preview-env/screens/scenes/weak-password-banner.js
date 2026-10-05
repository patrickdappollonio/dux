// The banner a signed-in browser gets when the password is weaker than today's
// minimums. dux refuses to store such a password, so the scene stores it with
// the minimums lowered, puts them back, and signs in with it: the first sign-in
// is when dux measures it and raises the banner.
const { clearToasts, expectBanner, goto } = require("../lib.js")
const { WEAK_PASSWORD, bannerClip, requireWeakPassword, resetAuth, signIn } = require("../auth.js")

module.exports = {
  file: "weak-password-banner.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requireWeakPassword()
    await goto(page, "")
    await signIn(page, WEAK_PASSWORD)
    await clearToasts(page)
    await expectBanner(page, "The dux password is weaker than the minimum it asks for.")
    return bannerClip(page, "weak-password-banner")
  },
}
