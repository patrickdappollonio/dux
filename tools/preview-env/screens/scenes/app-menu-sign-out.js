// The cog's app menu in a browser that signed in, with Sign out as its last
// item. The item exists only where there is a session to end, so the scene sets
// a password and signs in first.
const { boxOf, clearToasts, clickText, expectMenuOpen, goto, steadyBox } = require("../lib.js")
const { requirePassword, resetAuth, signIn } = require("../auth.js")

module.exports = {
  file: "app-menu-sign-out.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword()
    await goto(page, "")
    await signIn(page)
    await clearToasts(page)
    await clickText(page, "Settings")
    await expectMenuOpen(page, "Sign out")
    const menu = await steadyBox(() => boxOf(page, ['[role="menu"]']), { what: "the app menu" })
    // The cog it hangs off comes along, so the picture says where the menu is.
    const cog = await page.evaluate(() => {
      const b = [...document.querySelectorAll("button")].find(
        (x) => (x.textContent || "").trim() === "Settings" && x.getBoundingClientRect().width > 0,
      )
      if (!b) return null
      const r = b.getBoundingClientRect()
      return { x: r.x, y: r.y, right: r.right, bottom: r.bottom }
    })
    const pad = 16
    const left = Math.min(menu.x, cog ? cog.x : menu.x)
    const top = Math.min(menu.y, cog ? cog.y : menu.y)
    const right = Math.max(menu.x + menu.width, cog ? cog.right : 0)
    const x = Math.max(0, Math.round(left - pad))
    const y = Math.max(0, Math.round(top - pad))
    return {
      x,
      y,
      width: Math.min(1440 - x, Math.round(right + pad - x)),
      height: Math.min(900 - y, Math.round(menu.y + menu.height + pad - y)),
    }
  },
}
