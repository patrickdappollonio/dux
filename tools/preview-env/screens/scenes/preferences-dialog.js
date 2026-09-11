// The one place settings live, scrolled to the Tailscale row the server-mode
// docs point at.
const {
  boxOf,
  clearToasts,
  clickText,
  expectDialogOpen,
  expectVisibleText,
  goto,
  sleep,
} = require("../lib.js")

module.exports = {
  file: "preferences-dialog.png",
  viewport: "desktop",
  async shoot(page) {
    await goto(page, "")
    await clickText(page, "Settings")
    await clickText(page, "Preferences", "[role=menuitem],button")
    await sleep(1200)
    await page.evaluate(() => {
      const el = [...document.querySelectorAll("*")].find(
        (n) => n.children.length === 0 && /Bind your Tailscale address/i.test(n.textContent || ""),
      )
      el?.scrollIntoView({ block: "center" })
    })
    await sleep(900)
    await clearToasts(page)
    // The dialog, and the one row the server-mode docs point the reader at: a
    // scroll that landed somewhere else is a picture of a different setting.
    await expectDialogOpen(page, "Settings")
    await expectVisibleText(page, "Bind your Tailscale address", {
      what: "the Tailscale row the caption names",
    })
    return await boxOf(page, ['[role="dialog"]'])
  },
}
