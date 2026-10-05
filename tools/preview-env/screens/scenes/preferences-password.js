// The Password row at the head of Preferences, in a browser signed in with a
// password, with a new one half typed so the strength meter shows. Nothing is
// saved: the dialog is shot open and the browser closed.
const {
  boxOf,
  clearToasts,
  clickText,
  expectDialogOpen,
  expectVisible,
  expectVisibleText,
  goto,
  sleep,
  steadyBox,
} = require("../lib.js")
const { requirePassword, resetAuth, signIn } = require("../auth.js")

// A made-up passphrase that the meter rates, typed into a field that hides it.
const NEW_PASSWORD = "copper-meadow-lattice"

module.exports = {
  file: "preferences-password.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword()
    await goto(page, "")
    await signIn(page)
    await clickText(page, "Settings")
    await clickText(page, "Preferences", "[role=menuitem],button")
    await sleep(1200)
    const fresh = '[role="dialog"] input[autocomplete="new-password"]'
    await page.waitForSelector(fresh, { timeout: 15000 })
    await page.type(fresh, NEW_PASSWORD, { delay: 20 })
    await sleep(1200)
    await clearToasts(page)
    await expectDialogOpen(page, "Settings")
    await expectVisibleText(page, "Current password")
    await expectVisible(page, '[role="dialog"] [aria-label="Password strength"]', "the strength meter")
    // From the dialog's top to just under the row's last field, so the picture
    // is the row rather than every setting below it.
    const dialog = await steadyBox(() => boxOf(page, ['[role="dialog"]']), { what: "the dialog" })
    const fields = await boxOf(page, ['[role="dialog"] input[type="password"]'])
    return {
      x: dialog.x,
      y: dialog.y,
      width: dialog.width,
      height: Math.round(fields.y + fields.height + 24 - dialog.y),
    }
  },
}
