// The sidebar row's own actions menu, which is where every per-agent action
// lives.
const { agents, boxOf, clearToasts, goto, sleep, takeOver } = require("../lib.js")

module.exports = {
  file: "agent-session-menu.png",
  viewport: "desktop",
  async shoot(page) {
    const by = await agents()
    await goto(page, `#/agent/${by["fix-login-redirect"].id}`)
    await takeOver(page)
    await clearToasts(page)
    // The pane header carries a Session actions button too, so the row's is
    // picked by position (inside the sidebar) as well as by the row's own text.
    const opened = await page.evaluate(() => {
      const btn = [...document.querySelectorAll('[aria-label="Session actions"]')].find((b) => {
        if (b.getBoundingClientRect().x > 500) return false
        return /fix-login-redirect/.test(b.parentElement?.parentElement?.textContent || "")
      })
      if (!btn) return false
      btn.click()
      return true
    })
    if (!opened) throw new Error("no sidebar row menu for fix-login-redirect")
    await sleep(900)
    // Framed on the menu itself rather than to the bottom of the window: the row
    // it belongs to can be anywhere in the sidebar, and a menu near the bottom
    // opens upwards, so the menu's own extent is the only stable frame. The
    // sidebar comes along because the crop starts at the window's left edge.
    const menu = await boxOf(page, ['[role="menu"]'])
    const y = Math.max(0, Math.round(menu.y - 12))
    return {
      x: 0,
      y,
      width: Math.min(1440, Math.round(menu.x + menu.width + 12)),
      height: Math.min(900 - y, Math.round(menu.y + menu.height + 12 - y)),
    }
  },
}
