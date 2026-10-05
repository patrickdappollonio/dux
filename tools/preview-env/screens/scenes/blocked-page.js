// The page a blocked address gets when it opens dux: the small self-contained
// page dux serves to a fresh page load from a blocked address, which is what
// most blocked visitors ever see. The scene earns the block the way a guesser
// does: wrong passwords on the sign-in page, each one waiting out the slow-down
// the last one bought, until dux writes the address the preview's browser
// arrives from into blocked_addresses. Then it loads the page again. The page
// never names the address, so the picture carries none.
const { boxOf, clearToasts, expectVisibleText, goto, sleep, steadyBox } = require("../lib.js")
const { requirePassword, resetAuth } = require("../auth.js")

// The default `max_failed_logins` is five. An attempt that lands a moment
// before the slow-down is over is answered "wait" and counts for nothing, so
// there is room for a few of those.
const MAX_ATTEMPTS = 12

module.exports = {
  file: "blocked-page.png",
  viewport: "desktop",
  async shoot(page, { after }) {
    after(resetAuth)
    await requirePassword()
    await goto(page, "")
    const field = '[data-testid="login-form"] input[name="password"]'
    const submit = '[data-testid="login-form"] button[type="submit"]'
    const blocked = () =>
      page.evaluate(() => /This address is blocked/.test(document.body.innerText))
    for (let i = 0; i < MAX_ATTEMPTS && !(await blocked()); i++) {
      // The slow-down doubles from one second, so the longest wait here is
      // eight; the button is enabled again once it is over.
      await page.waitForFunction(
        (f, b) => {
          if (/This address is blocked/.test(document.body.innerText)) return true
          const input = document.querySelector(f)
          const button = document.querySelector(b)
          return input && button && !input.disabled && !button.disabled
        },
        { timeout: 30000 },
        field,
        submit,
      )
      // The page's countdown rounds to whole seconds, so it can re-enable the
      // button a little before dux will take the next attempt.
      await sleep(1200)
      if (await blocked()) break
      await page.type(field, `not-the-password-${i}`)
      await page.keyboard.press("Enter")
      await sleep(1500)
    }
    if (!(await blocked())) {
      throw new Error("blocked-page: the sign-in page never turned into the blocked page")
    }
    // A fresh load now, which dux answers with its own HTML page rather than the
    // app (it refuses the app's every asset too).
    await page.goto(page.url(), { waitUntil: "networkidle0", timeout: 45000 })
    await sleep(800)
    await clearToasts(page)
    await expectVisibleText(page, "This address is blocked", { selector: "h1" })
    await expectVisibleText(page, "blocked_addresses", { selector: "code" })
    return steadyBox(() => boxOf(page, ["main"], 56), { what: "the blocked page" })
  },
}
