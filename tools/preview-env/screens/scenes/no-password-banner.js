// The red banner every browser gets while dux has no password and is reachable
// beyond the machine it runs on. The preview listens on every interface with no
// password, so this is simply what it shows.
const { clearToasts, expectBanner, goto } = require("../lib.js")
const { bannerClip } = require("../auth.js")

module.exports = {
  file: "no-password-banner.png",
  viewport: "desktop",
  async shoot(page) {
    await goto(page, "")
    await clearToasts(page)
    await expectBanner(page, "No password: anyone who can reach this address can use dux.")
    return bannerClip(page, "no-auth-banner")
  },
}
